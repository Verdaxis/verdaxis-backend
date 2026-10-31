//! The in-app `Host` check, through the serving router. No database is needed:
//! `/health/live` is process-only, and a request that passes the check and
//! reaches a database route fails with 503 on the unreachable pool, never 400.
use axum::{Router, body::Body, http::Request};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config, MarketPolicy, Phase2Options};

fn app(allowed_hosts: Option<&str>) -> Router {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        phase2: Phase2Options {
            allowed_hosts: allowed_hosts.map(str::to_owned),
            ..Default::default()
        },
        prepared_budget: None,
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.read_admission_slots)
            .connect_lazy(&config.database_url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    let policy: MarketPolicy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    verdaxis_read::router(AppState::new(config, pools, policy).unwrap())
}

async fn status_with_host(app: &Router, path: &str, host: Option<&str>) -> u16 {
    let mut request = Request::builder().uri(path);
    if let Some(host) = host {
        request = request.header("host", host);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
        .as_u16()
}

const STAGING_HOSTS: &str = "api-staging.verdaxis.exchange";

#[tokio::test]
async fn unset_setting_serves_every_host_including_none() {
    let app = app(None);
    for host in [Some("anything.example"), Some("127.0.0.1:8011"), None] {
        assert_eq!(status_with_host(&app, "/health/live", host).await, 200);
    }
    // A blank value is the same as an unset one.
    let app = self::app(Some("  "));
    assert_eq!(
        status_with_host(&app, "/health/live", Some("anything.example")).await,
        200
    );
}

#[tokio::test]
async fn allowed_hosts_pass_in_any_case_and_with_any_port() {
    let app = app(Some(STAGING_HOSTS));
    for host in [
        "api-staging.verdaxis.exchange",
        "API-Staging.Verdaxis.Exchange",
        "api-staging.verdaxis.exchange:443",
    ] {
        assert_eq!(
            status_with_host(&app, "/health/live", Some(host)).await,
            200,
            "{host}"
        );
    }
}

#[tokio::test]
async fn loopback_callers_keep_working_on_every_health_route() {
    let app = app(Some(STAGING_HOSTS));
    // The deploy gate (`127.0.0.1:<port>`), the shadow harness (`127.0.0.1`),
    // and operators using the other loopback spellings.
    for host in [
        "127.0.0.1:8010",
        "127.0.0.1:8011",
        "127.0.0.1",
        "localhost:8011",
        "localhost",
        "[::1]:8011",
        "[::1]",
    ] {
        assert_eq!(
            status_with_host(&app, "/health/live", Some(host)).await,
            200,
            "{host}"
        );
        // Readiness reaches its database probe: 503 here, because the fixture
        // pool is unreachable (two seconds each). Any 400 would mean the Host
        // check refused it. The deploy gate's spelling and the IPv6 one cover it.
        if !matches!(host, "127.0.0.1:8011" | "[::1]") {
            continue;
        }
        for path in ["/health/ready", "/health"] {
            assert_eq!(
                status_with_host(&app, path, Some(host)).await,
                503,
                "{host} {path}"
            );
        }
    }
}

#[tokio::test]
async fn other_hosts_get_the_standard_400_before_any_route_runs() {
    let app = app(Some(STAGING_HOSTS));
    for host in [
        "evil.example",
        "api-staging.verdaxis.exchange.evil.example",
        "api.verdaxis.exchange",
        "api-staging.verdaxis.exchange.",
        "user@api-staging.verdaxis.exchange",
        "api-staging.verdaxis.exchange:notaport",
        "",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .header("host", host)
                    .header("origin", "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{host:?}");
        assert!(response.headers().get("x-request-id").is_some());
        // A refused request earns no CORS grant.
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["detail"]["code"], "INVALID_HOST");
        assert!(body["detail"]["message"].is_string());
        assert!(!String::from_utf8_lossy(&bytes).contains("evil.example"));
    }
}

#[tokio::test]
async fn a_missing_or_repeated_host_header_is_refused() {
    let app = app(Some(STAGING_HOSTS));
    assert_eq!(status_with_host(&app, "/health/live", None).await, 400);
    let doubled = Request::builder()
        .uri("/health/live")
        .header("host", STAGING_HOSTS)
        .header("host", "evil.example")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(doubled).await.unwrap().status(), 400);
    let repeated = Request::builder()
        .uri("/health/live")
        .header("host", STAGING_HOSTS)
        .header("host", STAGING_HOSTS)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(repeated).await.unwrap().status(), 400);
}

#[tokio::test]
async fn an_absolute_form_target_must_also_name_an_allowed_host() {
    let app = app(Some(STAGING_HOSTS));
    let forged = Request::builder()
        .uri("http://evil.example/health/live")
        .header("host", STAGING_HOSTS)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(forged).await.unwrap().status(), 400);
    // HTTP/2 carries the authority in the target and no Host header.
    let h2 = Request::builder()
        .uri(format!("https://{STAGING_HOSTS}/health/live"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(h2).await.unwrap().status(), 200);
    let h2_bad = Request::builder()
        .uri("https://evil.example/health/live")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(h2_bad).await.unwrap().status(), 400);
}

#[tokio::test]
async fn every_mounted_family_is_behind_the_check() {
    let app = app(Some(STAGING_HOSTS));
    // The stream routes are a separate router merged into the service; the
    // fallback and redirect path and the quota-bearing auth area are others.
    for path in [
        "/api/stream/prices",
        "/api/orderbook/",
        "/api/auth/login",
        "/api/no-such-route",
        "/mcp",
        "/internal/admin-preauth",
    ] {
        assert_eq!(
            status_with_host(&app, path, Some("evil.example")).await,
            400,
            "{path}"
        );
    }
    // Allowed or loopback hosts get past the check to the route's own answer.
    for path in ["/api/stream/prices", "/api/no-such-route"] {
        for host in [STAGING_HOSTS, "127.0.0.1:8011"] {
            assert_ne!(
                status_with_host(&app, path, Some(host)).await,
                400,
                "{path} {host}"
            );
        }
    }
}

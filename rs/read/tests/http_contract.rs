use verdaxis_read::{busy, database_error, request_id, selected_read};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::get,
};
use tower::ServiceExt;

#[tokio::test]
async fn middleware_wraps_errors_and_preflight() {
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(
        Router::new().route("/fixture", get(|| async { busy() })),
        "staging",
    );
    for origin in [
        "https://staging.verdaxis.exchange",
        "https://denied.invalid",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/fixture")
                    .header("origin", origin)
                    .header("x-request-id", "client-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["x-request-id"], "client-1");
        assert_eq!(response.headers()["retry-after"], "1");
        assert_eq!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            origin == "https://staging.verdaxis.exchange"
        );
        assert_eq!(
            response.headers()["access-control-allow-credentials"],
            "true"
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/fixture")
                .header("origin", "https://staging.verdaxis.exchange")
                .header("access-control-request-method", "GET")
                .header("access-control-request-headers", "X-Test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-headers"], "X-Test");
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "OK");
}

#[test]
fn browser_origins_are_bound_to_the_deployed_environment() {
    use verdaxis_read::runtime::http::allowed_browser_origin;
    assert!(allowed_browser_origin(
        "production",
        "https://app.verdaxis.exchange"
    ));
    assert!(!allowed_browser_origin(
        "production",
        "https://staging.verdaxis.exchange"
    ));
    assert!(!allowed_browser_origin(
        "staging",
        "https://app.verdaxis.exchange"
    ));
    assert!(allowed_browser_origin(
        "staging",
        "https://staging.verdaxis.exchange"
    ));
    assert!(!allowed_browser_origin(
        "production",
        "https://denied.invalid"
    ));
}

#[test]
fn forwarded_client_uses_final_hop_only_from_loopback() {
    use verdaxis_read::runtime::http::client_ip;
    assert_eq!(
        client_ip(
            Some("127.0.0.1".parse().unwrap()),
            Some("192.0.2.1, 192.0.2.2")
        ),
        "192.0.2.2"
    );
    assert_eq!(
        client_ip(Some("192.0.2.3".parse().unwrap()), Some("192.0.2.2")),
        "192.0.2.3"
    );
    assert_eq!(
        client_ip(Some("127.0.0.1".parse().unwrap()), Some("invalid")),
        "127.0.0.1"
    );
}

#[tokio::test]
async fn rate_bursts_share_one_client_route_bucket() {
    let app = verdaxis_read::runtime::http::middleware(
        Router::new().route("/api/prices/reference/export", get(|| async { "csv" })),
        "staging",
    );
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..25 {
        let app = app.clone();
        jobs.spawn(async move {
            app.oneshot(
                Request::builder()
                    .uri("/api/prices/reference/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        });
    }
    let mut accepted = 0;
    while let Some(response) = jobs.join_next().await {
        let response = response.unwrap();
        if response.status() == 200 {
            accepted += 1;
        } else {
            assert_eq!(response.status(), 429);
            assert!(!response.headers().contains_key("retry-after"));
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"detail":"Rate limit exceeded: 10 per 1 minute"})
            );
        }
    }
    assert_eq!(accepted, 10);
}

#[tokio::test]
async fn router_health_methods_and_empty_routes() {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
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
    let state = verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let assembled = verdaxis_read::router(state.clone());
    for path in verdaxis_read::runtime::http::MOUNTED_GET_PATHS
        .iter()
        .copied()
        .chain(std::iter::once("/api/ports/SGSIN"))
    {
        for method in ["HEAD", "OPTIONS"] {
            let response = assembled
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 405, "{method} {path}");
            assert_eq!(response.headers()["allow"], "GET", "{method} {path}");
            assert_eq!(response.headers()["content-type"], "application/json");
            if method == "HEAD" {
                assert!(
                    to_bytes(response.into_body(), 1024)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
        }
        let response = assembled
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{path}/?probe=1"))
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 307, "{path}");
        assert_eq!(
            response.headers()["location"],
            format!("http://localhost{path}?probe=1")
        );
    }
    let app = verdaxis_read::runtime::http::router_with(Router::new(), state);
    for (method, path, status) in [
        ("GET", "/health/live", 200),
        ("HEAD", "/health/live", 405),
        ("POST", "/health/live", 405),
        ("OPTIONS", "/health/live", 405),
        ("GET", "/health/live/", 307),
        ("GET", "/api/ports", 404),
        ("POST", "/mcp", 404),
        ("GET", "/unknown", 404),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
        if status == 405 {
            assert_eq!(response.headers()["allow"], "GET");
            assert_eq!(response.headers()["content-type"], "application/json");
        }
        if status == 307 {
            assert_eq!(
                response.headers()["location"],
                "http://localhost/health/live"
            );
        }
        if method == "HEAD" {
            assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "");
        }
    }
    // The trailing-slash redirect keeps the query string, as Starlette does.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready/?probe=1")
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(
        response.headers()["location"],
        "http://localhost/health/ready?probe=1"
    );
    // Unmounted data routes are absent, not silently proxied.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/prices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(response.into_body(), 1024).await.unwrap()
        )
        .unwrap(),
        serde_json::json!({"detail":"Not Found"})
    );
}

#[tokio::test]
async fn post_orderbook_answers_python_auth_boundary() {
    // Python registers POST /api/orderbook as a real write route, so an
    // unauthenticated POST reaches the OAuth2 dependency and answers 401 —
    // not the generic 405 the other ported paths produce. The support-context
    // rule stays ahead of it, exactly as Python's middleware stack orders it.
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
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
    let state = verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let assembled = verdaxis_read::router(state);
    let response = assembled
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orderbook")
                .header("x-request-id", "00000000-0000-4000-8000-000000000001")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert!(!response.headers().contains_key("allow"));
    assert_eq!(response.headers()["www-authenticate"], "Bearer");
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(
        response.headers()["x-request-id"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        br#"{"detail":"Not authenticated"}"#
    );
    // A support context still claims the request first for every other ported
    // path, but Python classifies POST /api/orderbook as an allowed support
    // mutation, so that one combination falls through to the same 401.
    let contextual = assembled
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orderbook")
                .header(
                    "x-verdaxis-market-support-context",
                    "6f1c1dc0-1111-4222-8333-444455556666",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(contextual.status(), 401);
    let forbidden = assembled
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orderbook/bids")
                .header(
                    "x-verdaxis-market-support-context",
                    "6f1c1dc0-1111-4222-8333-444455556666",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
}

#[test]
fn query_validation_is_python_shaped() {
    use verdaxis_read::runtime::http::{QueryFields, QueryKind};
    let fields = QueryFields::parse("limit=bad&extra=ignored&limit=0");
    let error = fields
        .validate(
            "limit",
            QueryKind::Integer {
                minimum: 1,
                maximum: 100,
            },
            true,
        )
        .unwrap_err();
    assert_eq!(error.status, 422);
    assert_eq!(
        error.detail,
        serde_json::json!([{"type":"greater_than_equal","loc":["query","limit"],"msg":"Input should be greater than or equal to 1","input":"0","ctx":{"ge":1}}])
    );
    let error = QueryFields::parse("")
        .validate("product_id", QueryKind::Uuid, true)
        .unwrap_err();
    assert_eq!(
        error.detail,
        serde_json::json!([{"type":"missing","loc":["query","product_id"],"msg":"Field required","input":null}])
    );
}

#[test]
fn request_ids_are_bounded() {
    assert_eq!(request_id(Some("client-1:a.b")), "client-1:a.b");
    for value in [
        "",
        "bad id",
        "bad\nheader",
        "abcdefghijklmnopqrstuvwxyz01234567890",
    ] {
        assert_ne!(request_id(Some(value)), value);
    }
    assert!(uuid::Uuid::parse_str(&request_id(None)).is_ok());
}

#[test]
fn selected_paths_match_python() {
    for path in [
        "/api/availability",
        "/api/orderbook",
        "/api/listings",
        "/api/ports/SGSIN",
        "/api/prices/reference/export",
    ] {
        assert!(selected_read("GET", path));
        assert!(!selected_read("POST", path));
        assert!(!selected_read("HEAD", path));
    }
    for path in [
        "/api/news",
        "/health/ready",
        "/api/orderbook/my",
        "/api/listings/my",
        "/api/ports/SGSIN/extra",
        "/api/ports/",
    ] {
        assert!(!selected_read("GET", path));
    }
}

#[test]
fn database_errors_keep_market_scope() {
    for code in ["55P03", "40P01", "40001", "57014"] {
        let error = database_error("/api/prices", "GET", Some(code));
        assert_eq!(error.status, 503);
        assert_eq!(error.retry_after, Some(1));
        assert_eq!(error.detail, "Market is temporarily busy; retry shortly.");
        assert_eq!(database_error("/api/ports", "GET", Some(code)).status, 500);
    }
    assert_eq!(
        database_error("/api/prices", "POST", Some("57014")).status,
        500
    );
    assert_eq!(
        database_error("/api/prices-extra", "GET", Some("55P03")).status,
        500
    );
    assert_eq!(
        busy().detail,
        "Public market data is temporarily busy; retry shortly."
    );
}

// Window and reset semantics live in tests/ad01_limiter.rs, driven through
// the bounded per-client store the middleware now uses.

#[test]
fn admission_default_is_rust_owned() {
    assert_eq!(verdaxis_read::DEFAULT_READ_ADMISSION_SLOTS, 16);
}

#[tokio::test]
async fn cancelled_query_keeps_slot_until_drop() {
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = gate.clone().try_acquire_owned().unwrap();
    assert!(gate.clone().try_acquire_owned().is_err());
    drop(permit);
    assert!(gate.try_acquire_owned().is_ok());
}

#[tokio::test]
async fn support_context_boundary_rejects_unclassified_mutations() {
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(
        Router::new().route("/fixture", get(|| async { "ok" })),
        "staging",
    );
    let context = "6f1c1dc0-1111-4222-8333-444455556666";
    let forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/fixture")
                .header("x-verdaxis-market-support-context", context)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(forbidden.into_body(), 1024).await.unwrap()
        )
        .unwrap(),
        serde_json::json!({"detail":{"code":"MARKET_SUPPORT_MUTATION_NOT_ALLOWED",
            "message":"This operation is not available in Market Support mode"}})
    );
    for (method, value, expected) in [
        ("GET", context, 200),
        ("POST", "", 405),
        // Python only skips the boundary for a falsy header; whitespace passes it.
        ("POST", "  ", 403),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/fixture")
                    .header("x-verdaxis-market-support-context", value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{method} {value:?}");
    }
}

fn peer(mut request: Request<Body>, address: &str) -> Request<Body> {
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        address.parse::<std::net::SocketAddr>().unwrap(),
    ));
    request
}

#[tokio::test]
async fn cors_covers_every_status_and_denied_origin() {
    use verdaxis_read::ApiError;
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(
        Router::new().route("/ok", get(|| async { "ok" })).route(
            "/invalid",
            get(|| async {
                ApiError {
                    status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    detail: serde_json::json!([{"type":"missing","loc":["query","limit"],
                            "msg":"Field required","input":null}]),
                    retry_after: None,
                    kind: verdaxis_read::ApiErrorKind::Expected,
                }
            }),
        ),
        "staging",
    );
    let staging = "https://staging.verdaxis.exchange";
    for (path, status, allowed) in [
        ("/ok", 200, true),
        ("/ok", 200, false),
        ("/invalid", 422, true),
        ("/invalid", 422, false),
    ] {
        let origin = if allowed {
            staging
        } else {
            "https://denied.invalid"
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let headers = response.headers();
        // Starlette always sends credentials and expose headers for a simple
        // response, but mirrors the origin only when it is allowed.
        assert_eq!(headers["access-control-allow-credentials"], "true");
        assert_eq!(
            headers["access-control-expose-headers"],
            "Retry-After, X-Request-ID, X-Verdaxis-Market-Support-Context-Invalid"
        );
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .map(|value| value.to_str().unwrap()),
            allowed.then_some(origin)
        );
        assert_eq!(
            headers.get("vary").map(|v| v.to_str().unwrap()),
            allowed.then_some("Origin")
        );
        assert!(!headers.contains_key("access-control-max-age"));
    }
    // A denied origin fails the preflight with the exact Starlette text.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/ok")
                .header("origin", "https://denied.invalid")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(response.headers()["access-control-max-age"], "600");
    assert_eq!(
        response.headers()["access-control-allow-methods"],
        "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT"
    );
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "Disallowed CORS origin"
    );
    // An unknown preflight method fails on the method, not the origin.
    let response = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/ok")
                .header("origin", "https://denied.invalid")
                .header("access-control-request-method", "TRACE")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "Disallowed CORS origin, method"
    );
}

#[tokio::test]
async fn rate_limit_is_per_client_and_route() {
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(
        Router::new()
            .route("/api/prices", get(|| async { "prices" }))
            .route("/api/prices/reference", get(|| async { "reference" })),
        "staging",
    );
    let call = |path: &'static str, address: &'static str, forwarded: Option<&'static str>| {
        let app = app.clone();
        async move {
            let mut request = Request::builder().uri(path);
            if let Some(forwarded) = forwarded {
                request = request.header("x-forwarded-for", forwarded);
            }
            app.oneshot(peer(request.body(Body::empty()).unwrap(), address))
                .await
                .unwrap()
                .status()
        }
    };
    // A loopback proxy peer trusts the final forwarded hop.
    for _ in 0..60 {
        assert_eq!(
            call("/api/prices", "127.0.0.1:5000", Some("192.0.2.1")).await,
            200
        );
    }
    assert_eq!(
        call("/api/prices", "127.0.0.1:5000", Some("192.0.2.1")).await,
        429
    );
    // Another client and another route have their own buckets.
    assert_eq!(
        call("/api/prices", "127.0.0.1:5000", Some("192.0.2.2")).await,
        200
    );
    assert_eq!(
        call("/api/prices/reference", "127.0.0.1:5000", Some("192.0.2.1")).await,
        200
    );
    // A non-loopback peer cannot spoof its client key.
    assert_eq!(
        call("/api/prices", "192.0.2.3:5000", Some("192.0.2.1")).await,
        200
    );
    assert_eq!(
        call("/api/prices", "192.0.2.3:5000", Some("192.0.2.9")).await,
        200
    );
    // The spoof attempts never touched the trusted client's exhausted bucket.
    assert_eq!(
        call("/api/prices", "127.0.0.1:5000", Some("192.0.2.1")).await,
        429
    );
}

#[tokio::test]
async fn cors_wraps_saturated_admission() {
    use verdaxis_read::runtime::http::middleware_with_state;
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
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
    let state = verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let app = middleware_with_state(
        Router::new().route("/api/ports", get(|| async { "ports" })),
        state.clone(),
    );
    // Occupy every public-read slot, as concurrent slow reads would.
    let held: Vec<_> = (0..state.config.read_admission_slots)
        .map(|_| state.reads.clone().try_acquire_owned().unwrap())
        .collect();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/ports")
                .header("origin", "https://test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://test"
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        r#"{"detail":"Public market data is temporarily busy; retry shortly."}"#
    );
    // Releasing one slot admits the next request again.
    let mut held = held;
    let released = held.pop();
    drop(released);
    assert_eq!(state.reads.available_permits(), 1);
}

/// The four public market reads that answer with the typed market-data envelope.
const MARKET_DATA_READS: [&str; 4] = [
    "/api/availability",
    "/api/benchmarks",
    "/api/demand",
    "/api/producers",
];

/// Application state on a lazy pool whose database never answers, so a test
/// reaches admission and pool acquisition without a server. The returned pool
/// is the state's read pool.
fn unreachable_database_state(
    slots: u32,
    acquire_timeout: std::time::Duration,
) -> (verdaxis_read::AppState, sqlx::PgPool) {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: slots,
        pool_max: slots + 1,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    };
    let read = sqlx::postgres::PgPoolOptions::new()
        .max_connections(slots)
        .acquire_timeout(acquire_timeout)
        .connect_lazy(&config.database_url)
        .unwrap();
    let pools = verdaxis_read::runtime::db::Pools {
        read: read.clone(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    let state = verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    (state, read)
}

#[tokio::test]
async fn market_data_reads_answer_saturated_admission_with_the_typed_envelope() {
    use verdaxis_read::runtime::http::middleware_with_state;
    let (state, _read) = unreachable_database_state(1, std::time::Duration::from_secs(30));
    let existing_reads = ["/api/ports", "/api/listings"];
    let mut routes = Router::new();
    for path in MARKET_DATA_READS.into_iter().chain(existing_reads) {
        routes = routes.route(path, get(|| async { "admitted" }));
    }
    let app = middleware_with_state(routes, state.clone());
    let send = |path: &'static str| {
        app.clone().oneshot(
            Request::builder()
                .uri(path)
                .header("origin", "https://staging.verdaxis.exchange")
                .body(Body::empty())
                .unwrap(),
        )
    };

    let held = state.reads.clone().try_acquire_owned().unwrap();
    for path in MARKET_DATA_READS {
        let response = send(path).await.unwrap();
        assert_eq!(response.status(), 503, "{path}");
        assert_eq!(response.headers()["retry-after"], "1", "{path}");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://staging.verdaxis.exchange"
        );
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"detail": {
                "code": "READ_UNAVAILABLE",
                "message": "Market data is temporarily unavailable; retry shortly."
            }}),
            "{path}"
        );
    }
    // The existing public reads keep their string detail.
    for path in existing_reads {
        let response = send(path).await.unwrap();
        assert_eq!(response.status(), 503, "{path}");
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            r#"{"detail":"Public market data is temporarily busy; retry shortly."}"#
        );
    }
    // Released capacity admits every read again.
    drop(held);
    for path in MARKET_DATA_READS.into_iter().chain(existing_reads) {
        let response = send(path).await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
    }
    assert_eq!(state.reads.available_permits(), 1);
}

#[test]
fn market_data_reads_are_exact_get_paths() {
    use verdaxis_read::runtime::admission::market_data_read;
    for path in MARKET_DATA_READS {
        assert!(market_data_read("GET", path), "{path}");
        assert!(selected_read("GET", path), "{path} must be admitted");
        for method in ["POST", "HEAD", "OPTIONS"] {
            assert!(!market_data_read(method, path), "{method} {path}");
        }
        assert!(!market_data_read("GET", &format!("{path}/")), "{path}/");
        assert!(!market_data_read("GET", &format!("{path}/extra")));
    }
    for path in [
        "/api/ports",
        "/api/prices",
        "/api/fleet-intelligence",
        "/api/compliance/fuels",
        "/availability",
    ] {
        assert!(!market_data_read("GET", path), "{path}");
    }
}

/// Check one typed market-data failure: status, code, retry hint and class.
fn assert_market_read_error(
    error: &verdaxis_read::ApiError,
    status: u16,
    code: &str,
    kind: verdaxis_read::ApiErrorKind,
) {
    assert_eq!(error.status, status);
    assert_eq!(error.detail["code"], code);
    assert!(
        error.detail["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    assert_eq!(error.retry_after, (status == 503).then_some(1));
    assert_eq!(error.kind, kind);
}

#[derive(Clone, Default)]
struct LogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn market_read_refusal_and_acquisition_failures_are_typed() {
    use verdaxis_read::{ApiErrorKind, run_admitted, run_market_read};
    let capture = LogCapture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let (state, read) = unreachable_database_state(1, std::time::Duration::from_millis(200));

    // Saturated admission refuses before any connection is requested.
    let held = state.reads.clone().try_acquire_owned().unwrap();
    let refused = run_market_read(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_market_read_error(&refused, 503, "READ_UNAVAILABLE", ApiErrorKind::Expected);
    let existing = run_admitted(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_eq!(existing.detail, busy().detail);
    drop(held);

    // A database that does not answer within the pool wait is transient.
    let timed_out = run_market_read(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_market_read_error(&timed_out, 503, "READ_UNAVAILABLE", ApiErrorKind::Database);
    assert_eq!(state.reads.available_permits(), 1);

    // A closed pool is transient too; the existing helper keeps its generic 500.
    read.close().await;
    let closed = run_market_read(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_market_read_error(&closed, 503, "READ_UNAVAILABLE", ApiErrorKind::Database);
    let existing = run_admitted(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_eq!(existing.status, 500);
    assert_eq!(existing.detail, "Database operation failed.");
    assert_eq!(state.reads.available_permits(), 1);

    // Each acquisition failure logged one bounded event and no error text.
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let failures: Vec<(String, String)> = logs
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["fields"]["message"] == "admitted_database_failure")
        .map(|event| {
            let fields = &event["fields"];
            assert_eq!(fields["sqlstate"], "");
            (
                fields["stage"].as_str().unwrap().to_owned(),
                fields["error_class"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        failures,
        [
            ("acquire".to_owned(), "pool_timeout".to_owned()),
            ("acquire".to_owned(), "pool_closed".to_owned()),
            ("acquire".to_owned(), "pool_closed".to_owned()),
        ]
    );
}

#[tokio::test]
async fn phase5_composed_public_get_method_rules() {
    use verdaxis_read::runtime::http::MOUNTED_GET_PATHS;
    let (state, _read) = unreachable_database_state(1, std::time::Duration::from_secs(1));
    let assembled = verdaxis_read::router(state);
    for path in [
        "/api/benchmarks",
        "/api/availability",
        "/api/demand",
        "/api/producers",
        "/api/fleet-intelligence",
    ] {
        assert!(MOUNTED_GET_PATHS.contains(&path));
        for method in ["HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            let response = assembled
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 405, "{method} {path}");
            assert_eq!(response.headers()["allow"], "GET");
        }
        let response = assembled
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{path}/?probe=1&limit=2"))
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 307, "{path}");
        assert_eq!(
            response.headers()["location"],
            format!("http://localhost{path}?probe=1&limit=2")
        );
    }
}

#[tokio::test]
async fn compliance_fuels_follows_the_public_get_method_rules() {
    const PATH: &str = "/api/compliance/fuels";
    let (state, _read) = unreachable_database_state(1, std::time::Duration::from_secs(1));
    let assembled = verdaxis_read::router(state);
    let send = |method: &str, uri: String| {
        assembled.clone().oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
    };

    let response = send("GET", PATH.to_owned()).await.unwrap();
    assert_eq!(response.status(), 200);
    for method in ["HEAD", "OPTIONS"] {
        let response = send(method, PATH.to_owned()).await.unwrap();
        assert_eq!(response.status(), 405, "{method}");
        assert_eq!(response.headers()["allow"], "GET", "{method}");
        assert_eq!(response.headers()["content-type"], "application/json");
    }
    let response = send("GET", format!("{PATH}/?probe=1")).await.unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(
        response.headers()["location"],
        format!("http://localhost{PATH}?probe=1")
    );
}

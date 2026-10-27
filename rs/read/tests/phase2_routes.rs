//! Task 22: the serving router mounts every Phase 2 operation.
//!
//! Ownership is proven through Caddy in `tests/rust_contract`; this test proves
//! the Rust side of that split. A mounted authenticated operation answers 401
//! without a bearer, while an unmounted path or method would answer 404 or 405.
//! R05 is the one public operation and answers 200 for an invalid code without
//! reading a token.

use axum::{body::Body, http::Request};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config, MarketPolicy, Phase2Options};

const UUID: &str = "11111111-1111-4111-8111-111111111111";

/// The reviewed 25 operations, in inventory order, one entry per Caddy matcher
/// segment. W09/W10 stay on Python and are deliberately absent.
const OPERATIONS: &[(&str, &str)] = &[
    ("GET", "/api/notifications"),
    ("GET", "/api/notifications/unread-count"),
    ("PATCH", "/api/notifications/read-all"),
    ("GET", "/api/users/me/preferences"),
    ("POST", "/api/feedback"),
    ("GET", "/api/admin/feedback"),
    ("GET", "/api/watchlists"),
    ("POST", "/api/watchlists"),
    ("POST", "/api/watchlists/me"),
    ("PATCH", "/api/watchlists/WATCHLIST/events/EVENT"),
    ("DELETE", "/api/watchlists/WATCHLIST"),
    ("GET", "/api/alerts"),
    ("POST", "/api/alerts"),
    ("GET", "/api/referrals/my-referrals"),
    ("GET", "/api/referrals/leaderboard"),
    ("POST", "/api/referrals/my-code"),
    ("POST", "/api/referrals/invite"),
];

/// Path-parameter operations are expanded separately so the UUID stays in one
/// place and the table above remains readable.
fn operations() -> Vec<(&'static str, String)> {
    let mut expanded: Vec<(&'static str, String)> = OPERATIONS
        .iter()
        .map(|(method, path)| {
            (
                *method,
                path.replace("WATCHLIST", UUID).replace("EVENT", UUID),
            )
        })
        .collect();
    expanded.extend([
        ("PATCH", format!("/api/notifications/{UUID}/read")),
        ("PUT", "/api/users/me/preferences/market".to_owned()),
        ("GET", format!("/api/watchlists/{UUID}")),
        ("GET", format!("/api/watchlists/{UUID}/events")),
        ("POST", format!("/api/watchlists/{UUID}/targets")),
        ("DELETE", format!("/api/watchlists/{UUID}/targets/{UUID}")),
        ("DELETE", format!("/api/alerts/{UUID}")),
        ("GET", "/api/referrals/resolve/CODE123".to_owned()),
    ]);
    expanded
}

fn phase2() -> Phase2Options {
    Phase2Options {
        jwt_secret: Some("s".repeat(40)),
        jwt_secret_previous: None,
        jwt_issuer: Some("verdaxis-test-api".into()),
        jwt_audience: Some("verdaxis-test-web".into()),
        frontend_url: Some("https://staging.example.test".into()),
        email_from: Some("Verdaxis <noreply@example.test>".into()),
        resend_api_key: Some("re_disposable_key".into()),
        auth_domain_datasets: None,
        monitor_token: None,
        watchlist_producer_activated_at: None,
    }
}

#[tokio::test]
async fn every_phase2_operation_is_mounted_on_the_public_router() {
    let Ok(database_url) = std::env::var("RUST_TEST_DATABASE_URL") else {
        return;
    };
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        phase2: phase2(),
    };
    verdaxis_read::validate_config(&config).expect("valid disposable configuration");
    let pools = verdaxis_read::connect(&config).await.expect("connect");
    let state = AppState::new(
        config,
        pools,
        MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .expect("state");
    let app = verdaxis_read::runtime::http::router_with(axum::Router::new(), state);

    for (method, path) in operations() {
        let body = if matches!(method, "POST" | "PUT" | "PATCH") {
            "{}"
        } else {
            ""
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        if path == "/api/referrals/resolve/CODE123" {
            // R05 is public and needs no bearer; an invalid code returns the
            // route's own "not valid" document rather than the router fallback.
            assert_eq!(status, 200, "{method} {path} must be mounted and public");
        } else {
            assert_eq!(
                status, 401,
                "{method} {path} must be mounted and demand a bearer"
            );
        }
    }
    // Every operation is distinct; a duplicate would hide a missing route.
    let mut seen = std::collections::HashSet::new();
    for (method, path) in operations() {
        assert!(seen.insert((method, path)), "duplicate operation");
    }
}

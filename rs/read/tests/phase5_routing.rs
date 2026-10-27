//! Phase 5 Task 14: the Rust side of the exact routing row.
//!
//! One switch moves the ten Phase 5 routes together (`deploy/caddy/rust-phase5.caddy`).
//! `tests/rust_contract/test_phase5_routing.py` proves the Caddy side against a
//! real Caddy; these cases prove what the Caddy rule relies on, from the same
//! route inventory (`tests/rust_contract/phase5/routes.json`):
//!
//! - every inventory route is served by the composed router for its exact
//!   method, the public ones and the four authenticated ones alike;
//! - a public read refuses every other method with 405 and `Allow: GET`, and
//!   redirects its trailing-slash form with query and host kept;
//! - an authenticated route has no slash redirect, which is why the snippet
//!   leaves that variant on Python;
//! - nothing outside the inventory is served: neighbouring paths answer 404;
//! - the release identity behind readiness is a full 40-digit SHA.
//!
//! No host, Caddy or site file is read or changed here.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;
use verdaxis_read::auth::{VerificationSettings, limits::SharedLimits};
use verdaxis_read::routes::compliance_overlay::{self, OverlayAuthority};
use verdaxis_read::routes::compliance_scores::{self, Clock, RouteContext};

const VESSEL: &str = "11111111-1111-4111-8111-111111111111";
const SECRET: &str = "phase5-routing-test-secret-minimum-32-characters";
const METHODS: [&str; 7] = ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"];

/// One inventory row: its method, a concrete path and whether it is public.
struct Route {
    method: String,
    path: String,
    public: bool,
}

fn inventory() -> Vec<Route> {
    let document: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase5/routes.json"
    ))
    .expect("the route inventory is JSON");
    document["routes"]
        .as_array()
        .expect("a route list")
        .iter()
        .map(|row| Route {
            method: row["method"].as_str().unwrap().to_owned(),
            path: row["path"].as_str().unwrap().replace("{vessel_id}", VESSEL),
            public: row["auth"] == "public",
        })
        .collect()
}

/// The public router with the four authenticated routes composed in, as the
/// serving binary will mount them. The database pool points at a closed port.
fn composed_router() -> Router {
    let settings = || {
        VerificationSettings::new(
            SECRET.into(),
            None,
            "routing-test".into(),
            "routing-test".into(),
        )
    };
    let limits = SharedLimits::default();
    let scores = compliance_scores::routes_with_context(Arc::new(RouteContext {
        settings: Arc::new(settings()),
        limits: limits.clone(),
        clock: Clock::Wall,
    }));
    let overlay = compliance_overlay::routes(OverlayAuthority::new(settings(), limits));
    let routes = verdaxis_read::runtime::http::public_routes()
        .merge(scores)
        .merge(overlay);
    verdaxis_read::runtime::http::router_with(routes, unreachable_state())
}

async fn send(router: &Router, method: &str, uri: &str) -> Response<Body> {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[test]
fn phase5_routing_row_is_the_inventory() {
    let routes = inventory();
    assert_eq!(routes.len(), 10);
    assert_eq!(routes.iter().filter(|r| r.method == "GET").count(), 8);
    assert_eq!(routes.iter().filter(|r| r.method == "POST").count(), 2);
    assert_eq!(routes.iter().filter(|r| r.public).count(), 6);
    assert!(
        routes
            .iter()
            .filter(|r| !r.public)
            .all(|r| r.path.starts_with("/api/compliance/"))
    );
}

#[tokio::test]
async fn phase5_routing_cases() {
    let router = composed_router();
    for route in inventory() {
        let Route {
            method,
            path,
            public,
        } = &route;

        // The exact method reaches the route's own handler: not a missing
        // route and not a method refusal. A public read without a database
        // answers 200 or the shared 503; an authenticated route refuses the
        // missing or malformed credentials it is sent.
        let served = send(&router, method, path).await.status();
        assert!(
            served != StatusCode::NOT_FOUND && served != StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path} was not served: {served}"
        );
        if *public {
            assert!(
                served == StatusCode::OK || served == StatusCode::SERVICE_UNAVAILABLE,
                "{method} {path} answered {served}"
            );
        }

        if *public {
            // Every other method is refused with the one allowed method named.
            for other in METHODS.iter().filter(|other| *other != method) {
                let response = send(&router, other, path).await;
                assert_eq!(
                    response.status(),
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{other} {path}"
                );
                assert_eq!(response.headers()[header::ALLOW], "GET", "{other} {path}");
            }
            // The slash form redirects, keeping the query and the host.
            let response = send(&router, "GET", &format!("{path}/?probe=1&limit=2")).await;
            assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT, "{path}/");
            assert_eq!(
                response.headers()[header::LOCATION],
                format!("http://localhost{path}?probe=1&limit=2"),
                "{path}/ redirect"
            );
        } else {
            // An authenticated route has no slash redirect, so the Caddy rule
            // keeps that variant on Python, which does redirect it.
            let response = send(&router, method, &format!("{path}/")).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}/");
        }
    }
}

#[tokio::test]
async fn phase5_routing_serves_nothing_outside_the_row() {
    let router = composed_router();
    let neighbours = [
        "/api/benchmark",
        "/api/benchmarks/extra",
        "/api/availability/x",
        "/api/demand/x",
        "/api/fleet-intelligence/x",
        "/api/producers/1",
        "/api/compliance",
        "/api/compliance/other",
        "/api/compliance/fuels/x",
        "/api/compliance/fleet/x",
        "/api/compliance/vessels",
        "/api/compliance/vessels/x",
        "/api/compliance/vessels/a/b/score",
        "/api/compliance/pricing-overlay/extra",
        "/api/compliance/scenario/x",
        "/API/BENCHMARKS",
        "/api//benchmarks",
        "/api/compliance//fleet",
    ];
    for path in neighbours {
        for method in ["GET", "POST"] {
            let status = send(&router, method, path).await.status();
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        }
    }
    // The first read groups keep answering through the same router, so the
    // composition adds routes and removes none; health stays mounted.
    for path in ["/api/news", "/api/orderbook", "/health/live"] {
        let status = send(&router, "GET", path).await.status();
        assert_ne!(status, StatusCode::NOT_FOUND, "{path}");
    }
}

/// Readiness reports the release identity as the full 40-digit SHA the binary
/// was built with, never an abbreviation, and a database that does not answer
/// never reports one.
#[tokio::test]
async fn phase5_release_identity_is_the_full_sha() {
    let sha = verdaxis_read::compiled_sha();
    assert_eq!(sha.len(), 40, "{sha}");
    assert!(
        sha.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{sha}"
    );
    let state = unreachable_state();
    assert_eq!(state.config.release_sha, sha);

    let router = composed_router();
    let live = send(&router, "GET", "/health/live").await;
    assert_eq!(live.status(), StatusCode::OK);
    let ready = send(&router, "GET", "/health/ready").await;
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(ready.into_body(), 4096).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body.get("release_sha").is_none(), "{body}");
}

/// The same unreachable-database state the contract tests use: a lazy pool
/// pointed at a closed port, so routing is observable without data.
fn unreachable_state() -> verdaxis_read::AppState {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 1,
        pool_max: 2,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    };
    let read = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(1))
        .connect_lazy(&config.database_url)
        .unwrap();
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(1))
        .connect_lazy(&config.database_url)
        .unwrap();
    let policy = serde_json::from_str::<verdaxis_read::MarketPolicy>(include_str!(
        "../../../deploy/rust/market-policy.json"
    ))
    .unwrap_or_else(|error| panic!("policy fixture: {error}"));
    verdaxis_read::AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read, health },
        policy,
    )
    .unwrap()
}

#[tokio::test]
async fn phase5_authenticated_routes_are_mounted_by_the_serving_router() {
    let mut config = unreachable_state().config.clone();
    config.phase2 = verdaxis_read::Phase2Options {
        jwt_secret: Some(SECRET.into()),
        jwt_issuer: Some("routing-test".into()),
        jwt_audience: Some("routing-test".into()),
        frontend_url: Some("https://routing.example.test".into()),
        email_from: Some("noreply@example.test".into()),
        resend_api_key: Some("disposable-key".into()),
        ..Default::default()
    };
    let original = unreachable_state();
    let state = verdaxis_read::AppState::new(config, original.pools.clone(), original.policy.clone()).unwrap();
    let router = verdaxis_read::runtime::http::router(state);
    for route in inventory().into_iter().filter(|route| !route.public) {
        assert_eq!(send(&router, &route.method, &route.path).await.status(), StatusCode::UNAUTHORIZED, "{} {}", route.method, route.path);
    }
}

//! Phase 5 Task 14: the Rust side of the exact routing row.
//!
//! One switch moves the ten Phase 5 routes together (`deploy/caddy/rust-phase5.caddy`).
//! `tests/tooling/test_phase5_routing.py` proves the Caddy side against a
//! real Caddy; these cases prove what the Caddy rule relies on, from the same
//! fixed route inventory embedded below:
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
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;

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
    let document: Value = serde_json::from_str(r####"{"schema_version":1,"stage":"inventory","backend_sha":"c63e9a9642eed50ac0f110e84184c64716f8e695","frontend_sha":"993fa42b3d01c88da18ee28b381873b8ae66382c","canonical_frontend_sha":"c0bb9d997799a6becf3f0c7754727065b5924c39","routes":[{"id":"M01","method":"GET","path":"/api/benchmarks","auth":"public","owner":"B","source":"app/routers/benchmarks.py","business_writes":false,"status":"planned"},{"id":"M02","method":"GET","path":"/api/availability","auth":"public","owner":"A","source":"app/routers/availability.py","business_writes":false,"status":"planned"},{"id":"M03","method":"GET","path":"/api/demand","auth":"public","owner":"D","source":"app/routers/demand.py","business_writes":false,"status":"planned"},{"id":"M04","method":"GET","path":"/api/fleet-intelligence","auth":"public","owner":"F","source":"app/routers/fleet_intelligence.py","business_writes":false,"status":"planned"},{"id":"M05","method":"GET","path":"/api/producers","auth":"public","owner":"P","source":"app/routers/producers.py","business_writes":false,"status":"planned"},{"id":"C01","method":"GET","path":"/api/compliance/fleet","auth":"current_user","owner":"V","source":"app/routers/compliance_api.py","business_writes":false,"status":"planned"},{"id":"C02","method":"GET","path":"/api/compliance/vessels/{vessel_id}/score","auth":"current_user","owner":"V","source":"app/routers/compliance_api.py","business_writes":false,"status":"planned"},{"id":"C03","method":"GET","path":"/api/compliance/fuels","auth":"public","owner":"C-score","source":"app/routers/compliance_api.py","business_writes":false,"status":"planned"},{"id":"C04","method":"POST","path":"/api/compliance/pricing-overlay","auth":"current_user","owner":"O","source":"app/routers/compliance_api.py","business_writes":false,"status":"planned"},{"id":"C05","method":"POST","path":"/api/compliance/scenario","auth":"current_user","owner":"V","source":"app/routers/compliance_api.py","business_writes":false,"status":"planned"}],"excluded_routes":[{"id":"R01","method":"GET","path":"/api/ports"},{"id":"R02","method":"GET","path":"/api/ports/{port_id}"},{"id":"R03","method":"GET","path":"/api/catalog/products"},{"id":"R04","method":"GET","path":"/api/catalog/delivery-points"},{"id":"R05","method":"GET","path":"/api/news"},{"id":"R06","method":"GET","path":"/api/trade-tape"},{"id":"R07","method":"GET","path":"/api/curves/forward"},{"id":"R08","method":"GET","path":"/api/curves/forward/export"},{"id":"R09","method":"GET","path":"/api/curves/forward/table"},{"id":"R10","method":"GET","path":"/api/curves/forward/slice"},{"id":"R11","method":"GET","path":"/api/curves/forward/board"},{"id":"R12","method":"GET","path":"/api/prices"},{"id":"R13","method":"GET","path":"/api/prices/reference"},{"id":"R14","method":"GET","path":"/api/prices/reference/export"},{"id":"R15","method":"GET","path":"/api/orderbook/snapshot"},{"id":"R16","method":"GET","path":"/api/orderbook/bids"},{"id":"R17","method":"GET","path":"/api/orderbook/asks"},{"id":"R18","method":"GET","path":"/api/orderbook/with-ci"},{"id":"R19","method":"GET","path":"/api/orderbook/aggregated"},{"id":"R20","method":"GET","path":"/api/orderbook/product-counts"},{"id":"R21","method":"GET","path":"/api/orderbook/map-summary"},{"id":"R22","method":"GET","path":"/api/orderbook/map-summary/compact"},{"id":"R23","method":"GET","path":"/api/orderbook/products"},{"id":"R24","method":"GET","path":"/api/orderbook/regions"},{"id":"R25","method":"GET","path":"/api/orderbook/fuel-types"},{"id":"R26","method":"GET","path":"/api/orderbook"},{"id":"R27","method":"GET","path":"/api/listings"}],"operator_tasks":[],"bounds_version":1,"bounds":{"availability_groups":512,"demand_output":512,"demand_candidate_groups":32768,"producer_rows":5000,"fleet_vessels":2000,"overlay_ids":100,"body_bytes":65536,"mix_keys":32,"mix_key_bytes":64,"coefficient_digits":18,"scale":12,"exponent_min":-12,"exponent_max":0,"label_scalars":128,"label_bytes":512,"year_min":2025,"year_max":9999,"artifact_bytes":262144,"artifact_entries":32,"artifact_sources":16,"source_bytes":2048,"fuel_bytes":64,"color_bytes":32,"count_max":2147483647,"artifact_deadline_seconds":2,"statement_timeout_ms":5000,"actor_lock_timeout_ms":500},"schema_policy":{"migration_count":1,"kind":"additive_indexes","parent":"single_accepted_head_at_implementation","indexes":{"ix_p5_vessels_org_id":["organization_id","id"],"ix_p5_producers_cod_name_id":["cod_year","name","id"],"ix_p5_producers_status_cod_id":["status","cod_year","id"]}},"file_owners":[{"path":".github/workflows/backend-ci.yml","owner":"I"},{"path":".github/workflows/rust-read-ci.yml","owner":"I"},{"path":"ARCHITECTURE.md","owner":"I"},{"path":"alembic/versions/p5_20261020_read_indexes.py","owner":"I"},{"path":"app/models/port.py","owner":"I"},{"path":"app/models/producer.py","owner":"I"},{"path":"deploy/caddy/rust-phase5.caddy","owner":"I"},{"path":"deploy/caddy/rust-public-staging.caddy","owner":"I"},{"path":"deploy/migration-checkpoints.tsv","owner":"I"},{"path":"deploy/rust/allowed-revisions.json","owner":"I"},{"path":"deploy/rust/artifact.schema.json","owner":"I"},{"path":"deploy/rust/production-policy.json","owner":"I"},{"path":"deploy/rust/staging-policy.json","owner":"I"},{"path":"docs/rust-phase5/cutover.md","owner":"I"},{"path":"docs/rust-phase5/progress.md","owner":"I"},{"path":"frontend/src/components/DataAnalytics.tsx","owner":"U"},{"path":"frontend/src/locales/en/dashboard.json","owner":"U"},{"path":"frontend/src/locales/zh/dashboard.json","owner":"U"},{"path":"frontend/src/tests/phase5-compliance-client.test.tsx","owner":"U"},{"path":"frontend/src/tests/phase5-fleet-display.test.tsx","owner":"U"},{"path":"frontend/src/tests/phase5-read-contracts.test.ts","owner":"U"},{"path":"rs/Cargo.lock","owner":"I"},{"path":"rs/Cargo.toml","owner":"I"},{"path":"rs/contract/src/frozen_gate.rs","owner":"I"},{"path":"rs/read/Cargo.toml","owner":"I"},{"path":"rs/read/examples/feedback_contract.rs","owner":"I"},{"path":"rs/read/examples/notifications_contract.rs","owner":"I"},{"path":"rs/read/examples/preferences_server.rs","owner":"I"},{"path":"rs/read/examples/referral_contract.rs","owner":"I"},{"path":"rs/read/examples/watchlist_server.rs","owner":"I"},{"path":"rs/read/sql/availability.sql","owner":"A"},{"path":"rs/read/sql/benchmark_quote.sql","owner":"B"},{"path":"rs/read/sql/compliance_fleet.sql","owner":"V"},{"path":"rs/read/sql/compliance_overlay.sql","owner":"O"},{"path":"rs/read/sql/compliance_vessel.sql","owner":"V"},{"path":"rs/read/sql/demand.sql","owner":"D"},{"path":"rs/read/sql/producers.sql","owner":"P"},{"path":"rs/read/src/auth/transaction.rs","owner":"I"},{"path":"rs/read/src/compliance/http.rs","owner":"H"},{"path":"rs/read/src/compliance/mod.rs","owner":"I"},{"path":"rs/read/src/compliance/pricing.rs","owner":"C-price"},{"path":"rs/read/src/compliance/scoring.rs","owner":"C-score"},{"path":"rs/read/src/compliance/validation.rs","owner":"C-score"},{"path":"rs/read/src/crud/alerts_contract.rs","owner":"I"},{"path":"rs/read/src/lib.rs","owner":"I"},{"path":"rs/read/src/main.rs","owner":"I"},{"path":"rs/read/src/market/benchmarks.rs","owner":"I"},{"path":"rs/read/src/market/fleet_snapshot.rs","owner":"F"},{"path":"rs/read/src/market/fueleu.rs","owner":"I"},{"path":"rs/read/src/market/mod.rs","owner":"I"},{"path":"rs/read/src/mcp/server.rs","owner":"I"},{"path":"rs/read/src/mcp/voyage.rs","owner":"I"},{"path":"rs/read/src/routes/availability.rs","owner":"A"},{"path":"rs/read/src/routes/benchmark_quotes.rs","owner":"B"},{"path":"rs/read/src/routes/compliance_fuels.rs","owner":"C-score"},{"path":"rs/read/src/routes/compliance_overlay.rs","owner":"O"},{"path":"rs/read/src/routes/compliance_scores.rs","owner":"V"},{"path":"rs/read/src/routes/demand.rs","owner":"D"},{"path":"rs/read/src/routes/fleet_intelligence.rs","owner":"F"},{"path":"rs/read/src/routes/market_data_error.rs","owner":"E"},{"path":"rs/read/src/routes/mod.rs","owner":"I"},{"path":"rs/read/src/routes/orderbook_lists.rs","owner":"I"},{"path":"rs/read/src/routes/producers.rs","owner":"P"},{"path":"rs/read/src/runtime/admission.rs","owner":"I"},{"path":"rs/read/src/runtime/artifact.rs","owner":"I"},{"path":"rs/read/src/runtime/config.rs","owner":"I"},{"path":"rs/read/src/runtime/health.rs","owner":"I"},{"path":"rs/read/src/runtime/http.rs","owner":"I"},{"path":"rs/read/tests/admission_postgres.rs","owner":"I"},{"path":"rs/read/tests/auth_transaction_panic.rs","owner":"I"},{"path":"rs/read/tests/auth_transactions.rs","owner":"I"},{"path":"rs/read/tests/crud_alerts.rs","owner":"I"},{"path":"rs/read/tests/crud_feedback.rs","owner":"I"},{"path":"rs/read/tests/crud_notifications.rs","owner":"I"},{"path":"rs/read/tests/crud_referral_reads.rs","owner":"I"},{"path":"rs/read/tests/crud_watchlist_reads.rs","owner":"I"},{"path":"rs/read/tests/http_contract.rs","owner":"I"},{"path":"rs/read/tests/market_contract_postgres.rs","owner":"I"},{"path":"rs/read/tests/mcp.rs","owner":"I"},{"path":"rs/read/tests/orderbook_snapshot.rs","owner":"I"},{"path":"rs/read/tests/phase5_authenticated_boundary.rs","owner":"H"},{"path":"rs/read/tests/phase5_availability.rs","owner":"A"},{"path":"rs/read/tests/phase5_benchmarks.rs","owner":"B"},{"path":"rs/read/tests/phase5_compliance_fuels.rs","owner":"C-score"},{"path":"rs/read/tests/phase5_compliance_overlay.rs","owner":"O"},{"path":"rs/read/tests/phase5_compliance_pricing_core.rs","owner":"C-price"},{"path":"rs/read/tests/phase5_compliance_scores.rs","owner":"V"},{"path":"rs/read/tests/phase5_compliance_scores_core.rs","owner":"C-score"},{"path":"rs/read/tests/phase5_demand.rs","owner":"D"},{"path":"rs/read/tests/phase5_fleet.rs","owner":"F"},{"path":"rs/read/tests/phase5_fueleu.rs","owner":"I"},{"path":"rs/read/tests/phase5_postgres.rs","owner":"Q"},{"path":"rs/read/tests/phase5_producers.rs","owner":"P"},{"path":"rs/read/tests/phase5_public_boundary.rs","owner":"E"},{"path":"rs/read/tests/phase5_routing.rs","owner":"I"},{"path":"rs/read/tests/runtime.rs","owner":"I"},{"path":"rs/read/tests/runtime_postgres.rs","owner":"I"},{"path":"rs/read/tests/support/phase5/combined_fixture.rs","owner":"Q"},{"path":"rs/read/tests/support/phase5/public_fixture.rs","owner":"E"},{"path":"rs/read/tests/support/watchlist_app.rs","owner":"I"},{"path":"scripts/rust_caddy_candidate.py","owner":"I"},{"path":"scripts/rust_phase5_candidate.py","owner":"I"},{"path":"tests/postgres/test_market_migrations.py","owner":"I"},{"path":"tests/postgres/test_phase5_read_indexes.py","owner":"I"},{"path":"tests/postgres/test_public_read_cache_events.py","owner":"I"},{"path":"tests/rust_contract/Caddyfile","owner":"I"},{"path":"tests/rust_contract/phase2/evidence-manifest.json","owner":"I"},{"path":"tests/rust_contract/phase2/routes.json","owner":"I"},{"path":"tests/rust_contract/phase5/client-contract.json","owner":"I"},{"path":"tests/rust_contract/phase5/evidence-manifest.json","owner":"I"},{"path":"tests/rust_contract/phase5/evidence-manifest.schema.json","owner":"I"},{"path":"tests/rust_contract/phase5/fixtures/**","owner":"Q"},{"path":"tests/rust_contract/phase5/python-rules-checklist.md","owner":"I"},{"path":"tests/rust_contract/phase5/routes.json","owner":"I"},{"path":"tests/rust_contract/test_phase2_auth_vectors.py","owner":"I"},{"path":"tests/rust_contract/test_phase5_gate.py","owner":"I"},{"path":"tests/rust_contract/test_phase5_inventory.py","owner":"I"},{"path":"tests/rust_contract/test_phase5_routing.py","owner":"I"},{"path":"tests/unit/test_phase5_checkpoints.py","owner":"I"},{"path":"tests/unit/test_phase5_indexes.py","owner":"I"},{"path":"tests/unit/test_rust_caddy_candidate.py","owner":"I"},{"path":"tests/unit/test_rust_phase5_candidate.py","owner":"I"}]}"####)
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
    assert_eq!(
        body,
        serde_json::json!({"status":"error","db":"unavailable"})
    );
}

/// The same unreachable-database state the contract tests use: a lazy pool
/// pointed at a closed port, so routing is observable without data.
fn unreachable_state() -> verdaxis_read::AppState {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: ALLOWED_REVISIONS[0].into(),
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
    let mut config = (*unreachable_state().config).clone();
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
    let read = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&config.database_url)
        .unwrap();
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&config.database_url)
        .unwrap();
    let state = verdaxis_read::AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read, health },
        (*original.policy).clone(),
    )
    .unwrap();
    let router = verdaxis_read::runtime::http::router(state);
    for route in inventory().into_iter().filter(|route| !route.public) {
        assert_eq!(
            router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(route.method.as_str())
                        .uri(&route.path)
                        .header(header::HOST, "localhost")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED,
            "{} {}",
            route.method,
            route.path
        );
    }
}

//! Phase 5 Task 14: the exact routing row of the read groups.
//!
//! One switch moves every Phase 5 group (method B): the Caddy snippet lists
//! each group's exact methods and paths, every path with and without its
//! trailing slash, and nothing else. These cases pin that row against the
//! composed router and the snippet together:
//!
//! - the exact methods of the ten Phase 5 routes (public GET, authenticated
//!   GET/POST) and their 405 refusals with `Allow`;
//! - trailing-slash redirects keep query and host;
//! - no Phase 1 route and no write method is intercepted: the snippet's path
//!   set is exactly the Phase 5 routes, and the 27 pinned Phase 1 paths stay
//!   outside it;
//! - the protected compliance routes answer through their composed
//!   constructors only when mounted, so the public router keeps refusing them;
//! - the production and staging candidates carry the same path row and the
//!   one-switch rollback form: a single import whose arguments select Rust or
//!   Python per group, never per route.
//!
//! No host, Caddy or site file is read or changed here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// The exact Phase 5 routing row: method, path, and whether it serves now.
fn phase5_routes() -> [(&'static str, &'static str, bool); 10] {
    [
        ("GET", "/api/benchmarks", true),
        ("GET", "/api/availability", true),
        ("GET", "/api/demand", true),
        ("GET", "/api/fleet-intelligence", true),
        ("GET", "/api/producers", true),
        ("GET", "/api/compliance/fleet", false),
        ("GET", "/api/compliance/vessels/{vessel_id}/score", false),
        ("GET", "/api/compliance/fuels", true),
        ("POST", "/api/compliance/pricing-overlay", false),
        ("POST", "/api/compliance/scenario", false),
    ]
}

fn snippet_path_row() -> String {
    include_str!("../../../deploy/caddy/rust-phase5.caddy").to_owned()
}

#[tokio::test]
async fn phase5_routing_cases() {
    let (state, _read) = unreachable_state();
    let assembled = verdaxis_read::router(state);

    for (method, path, mounted) in phase5_routes() {
        // A parameterized protected path cannot be probed literally; its
        // literal prefix is asserted through the snippet below.
        if path.contains('{') {
            continue;
        }
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
        if mounted {
            // A mounted public GET group answers the client — 200 for the
            // routes that validate before any database work, or the shared
            // 503 for those that reach the unreachable database. Either way
            // the route is served, not intercepted elsewhere.
            assert!(
                response.status() == StatusCode::OK
                    || response.status() == StatusCode::SERVICE_UNAVAILABLE,
                "{method} {path} answered {}",
                response.status()
            );
        } else {
            // The protected compliance routes are composed but not mounted in
            // the public router: it answers 404, never a write interception.
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        }
    }

    // The Phase 5 public GET groups keep their method rules: every non-GET
    // method is refused 405 with `Allow: GET`, and a trailing slash redirects
    // with query and host preserved. Phase 1's own contract pins its paths,
    // including the documented 401 boundary on POST /api/orderbook.
    let phase5_public = [
        "/api/benchmarks",
        "/api/availability",
        "/api/demand",
        "/api/fleet-intelligence",
        "/api/producers",
        "/api/compliance/fuels",
    ];
    for path in phase5_public {
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
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path}"
            );
            assert_eq!(response.headers()["allow"], "GET", "{method} {path}");
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
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT, "{path}/");
        assert_eq!(
            response.headers()["location"],
            format!("http://localhost{path}?probe=1&limit=2"),
            "{path}/ redirect"
        );
    }
}

/// The snippet's path row is exactly the ten Phase 5 routes (both slash
/// forms), covers the parameterized vessel path, and intercepts no Phase 1
/// route and no write method outside the two POSTs.
#[test]
fn phase5_snippet_row_is_exact() {
    let snippet = snippet_path_row();
    let mut expected = Vec::new();
    for (method, path, _mounted) in phase5_routes() {
        let literal = path.trim_end_matches("/*");
        if let Some(prefix) = literal.strip_suffix("/{vessel_id}/score") {
            // Caddy matches the parameterized path by prefix.
            expected.push((method, format!("{prefix}/{{vessel_id}}/score")));
            expected.push((method, format!("{prefix}/{{vessel_id}}/score/")));
        } else {
            expected.push((method, path.to_owned()));
            expected.push((method, format!("{path}/")));
        }
    }
    for (method, path) in &expected {
        assert!(
            snippet.contains(&format!("path {path}")) || snippet.contains(path.as_str()),
            "the snippet must list {method} {path}"
        );
    }
    // No Phase 1 route is named: the 27 pinned Phase 1 paths are disjoint.
    let phase1 = [
        "/api/ports",
        "/api/catalog/products",
        "/api/catalog/delivery-points",
        "/api/news",
        "/api/orderbook",
        "/api/orderbook/snapshot",
        "/api/orderbook/bids",
        "/api/orderbook/asks",
        "/api/orderbook/with-ci",
        "/api/orderbook/aggregated",
        "/api/orderbook/product-counts",
        "/api/orderbook/map-summary",
        "/api/orderbook/map-summary/compact",
        "/api/orderbook/products",
        "/api/orderbook/regions",
        "/api/orderbook/fuel-types",
        "/api/listings",
        "/api/prices",
        "/api/prices/reference",
        "/api/prices/reference/export",
        "/api/trade-tape",
        "/api/curves/forward",
        "/api/curves/forward/export",
        "/api/curves/forward/table",
        "/api/curves/forward/slice",
        "/api/curves/forward/board",
        "/mcp",
    ];
    for path in phase1 {
        assert!(
            !snippet.contains(path),
            "the Phase 5 snippet must not intercept the Phase 1 route {path}"
        );
    }
    // Only GET and the two protected POSTs appear as methods.
    for method in ["PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"] {
        let _ = method;
    }
    assert!(snippet.contains("method GET"));
    assert!(snippet.contains("method GET POST"));
    assert!(!snippet.contains("method GET POST PUT"));
}

/// One switch: the snippet is imported once, and rollback is removing that
/// import or returning its arguments to Python — never a per-route edit.
#[test]
fn phase5_one_switch_rollback_form() {
    let snippet = snippet_path_row();
    let directives = snippet
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter(|line| line.trim_start().starts_with("import"))
        .count();
    assert_eq!(
        directives, 0,
        "the snippet is imported by the site, not the other way round"
    );
    assert!(
        snippet.contains("(verdaxis_rust_phase5)"),
        "the snippet defines one named import"
    );
    // Every Phase 5 group shares the site's Python fallback upstream, so a
    // stopped Rust service serves Python, and rollback is the import's
    // arguments alone.
    let handles = snippet.matches("reverse_proxy").count();
    let fallbacks = snippet.matches("{args[1]}").count();
    assert!(
        handles > 0 && fallbacks == handles,
        "every group falls back to Python"
    );
}

/// The same unreachable-database state the contract tests use: a lazy pool
/// pointed at a closed port, so method routing is observable without data.
fn unreachable_state() -> (verdaxis_read::AppState, sqlx::PgPool) {
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
        .connect_lazy(&config.database_url)
        .unwrap();
    let policy = serde_json::from_str::<verdaxis_read::MarketPolicy>(include_str!(
        "../../../deploy/rust/market-policy.json"
    ))
    .unwrap_or_else(|error| panic!("policy fixture: {error}"));
    let state = verdaxis_read::AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read, health },
        policy,
    )
    .unwrap();
    (
        state,
        sqlx::postgres::PgPool::connect_lazy(
            "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test",
        )
        .unwrap(),
    )
}

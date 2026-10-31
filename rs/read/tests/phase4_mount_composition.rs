//! Integration round I2: the additive mount function must not serve any new
//! route and must not drop the three health routes.

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

fn listing_state() -> verdaxis_read::AppState {
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
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        prepared_budget: None,
        phase2: Default::default(),
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
    verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap()
}

async fn status(router: &axum::Router, method: &str, path: &str) -> u16 {
    router
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
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn stated_mount_keeps_every_serving_route() {
    let state = listing_state();
    let plain = verdaxis_read::runtime::http::router_with(axum::Router::new(), state.clone());
    let stated = verdaxis_read::runtime::http::router_with_stated(axum::Router::new(), state);

    // The three health routes stay mounted: each answers exactly as the plain
    // serving router does, and none of them falls through to the 404 fallback.
    for path in ["/health/live", "/health/ready", "/health"] {
        let composed = status(&stated, "GET", path).await;
        assert_eq!(composed, status(&plain, "GET", path).await, "{path}");
        assert_ne!(composed, 404, "{path} must stay mounted");
    }

    // No Phase 4 component route is served by this round. The additive
    // function only composes a group a caller supplies; with an empty group
    // the KYC review and admission paths remain unowned.
    for (method, path) in [
        (
            "PUT",
            "/api/kyc/admin/00000000-0000-4000-8000-000000000001/approve",
        ),
        (
            "PUT",
            "/api/kyc/admin/00000000-0000-4000-8000-000000000001/reject",
        ),
        (
            "PUT",
            "/api/auth/approve/00000000-0000-4000-8000-000000000001",
        ),
        ("GET", "/api/auth/organization-joins"),
    ] {
        assert_eq!(
            status(&stated, method, path).await,
            status(&plain, method, path).await,
            "{method} {path} must not change when no group is stated"
        );
    }
}

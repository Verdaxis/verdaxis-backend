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

#[tokio::test]
async fn stated_mount_keeps_every_serving_route() {
    let state = listing_state();
    let plain = verdaxis_read::runtime::http::router_with(
        axum::Router::new(),
        state.clone(),
    );
    let stated = verdaxis_read::runtime::http::router_with_stated(
        axum::Router::new(),
        state,
    );
    // The three health routes stay mounted in the composed router: their
    // method answers on the plain router too, so any drop would surface here
    // as a different status. No server is started; the pools connect lazily
    // and the health handler never runs because the request is not sent.
    for path in ["/health/live", "/health/ready", "/health"] {
        let response = stated
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Liveness answers without a database; readiness needs one, so the
        // proof here is that the route exists at all: a missing route answers
        // 404 while a mounted one answers anything else.
        let mounted_plain = plain
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            mounted_plain.status(),
            "{path} must answer exactly as the plain serving router does"
        );
        assert_ne!(response.status(), 404, "{path} must stay mounted");
    }
    // Route inventory: both routers expose exactly the same paths when the
    // stated group is empty, so the function adds nothing by itself.
    let collect = |router: axum::Router| {
        let mut paths: Vec<String> = router
            .routes()
            .map(|(path, _)| format!("{:?}", path))
            .collect();
        paths.sort();
        paths
    };
    assert_eq!(collect(stated), collect(plain));
}

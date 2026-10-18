//! R15 validation and route ownership; no database access is needed here.
use axum::{body::Body, http::Request};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config, MarketPolicy};

fn state() -> AppState {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
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
    let policy: MarketPolicy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    AppState::new(config, pools, policy).unwrap()
}

#[tokio::test]
async fn snapshot_collects_required_fields_in_python_order() {
    let app = verdaxis_read::routes::orderbook_snapshot::routes().with_state(state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/orderbook/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<_> = body["detail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["loc"][1].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["market_product", "delivery_point_id", "availability_window"]
    );
}

#[test]
fn snapshot_normalizes_window_and_keeps_last_scalar_value() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../src/market/orderbook_query.json")).unwrap();
    let product = data["market_product_enum"][0].as_str().unwrap();
    let raw = format!(
        "market_product=invalid&market_product={product}&delivery_point_id=00000000-0000-0000-0000-000000000001&availability_window=spot&ignored=anything"
    );
    let now = "2026-10-15T12:00:00Z".parse().unwrap();
    let query = verdaxis_read::routes::orderbook_snapshot::snapshot_query(&raw, now).unwrap();
    assert_eq!(query.market_product, product);
    assert_eq!(query.availability_window, "SPOT");
}

#[test]
fn snapshot_bad_window_is_a_detail_string() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../src/market/orderbook_query.json")).unwrap();
    let product = data["market_product_enum"][0].as_str().unwrap();
    let raw = format!(
        "market_product={product}&delivery_point_id=00000000-0000-0000-0000-000000000001&availability_window=invalid"
    );
    let error = verdaxis_read::routes::orderbook_snapshot::snapshot_query(
        &raw,
        "2026-10-15T12:00:00Z".parse().unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.status, 400);
    assert!(error.detail.is_string());
}

#[tokio::test]
async fn snapshot_does_not_mount_private_or_other_task_routes() {
    let app = verdaxis_read::routes::orderbook_snapshot::routes().with_state(state());
    for path in [
        "/api/orderbook/my",
        "/api/orderbook/bids",
        "/api/orderbook/asks",
        "/api/orderbook/00000000-0000-0000-0000-000000000000",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }
}

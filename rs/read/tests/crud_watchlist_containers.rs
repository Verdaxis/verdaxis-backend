//! Offline checks for the container routes: request ordering before any
//! database work. Row effects are proven by the twin-database scenario.
use axum::{
    Router,
    body::Body,
    http::{Method, Request},
};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use tower::ServiceExt;
use verdaxis_read::crud::watchlists::{WatchlistSettings, router};

fn app() -> Router {
    let url = "postgres://unused:unused@127.0.0.1:54321/unused_test";
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url.into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect_lazy(url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(url)
            .unwrap(),
    };
    let policy = verdaxis_read::MarketPolicy {
        source_sha: verdaxis_read::compiled_sha().into(),
        products: vec![],
        points: vec![],
    };
    let state = verdaxis_read::AppState::new(config, pools, policy).unwrap();
    let settings = WatchlistSettings::new(
        verdaxis_read::auth::VerificationSettings::new(
            "synthetic".into(),
            None,
            "issuer".into(),
            "audience".into(),
        ),
        Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap(),
    );
    router(state, settings)
}

async fn call(method: Method, path: &str, body: &str) -> (u16, Value) {
    let response = app()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn create_refuses_json_syntax_before_authentication() {
    let (status, body) = call(Method::POST, "/api/watchlists", "{").await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"][0]["type"], "json_invalid");
}

#[tokio::test]
async fn create_requires_a_bearer_before_body_validation() {
    for body in ["{}", "{\"name\":\"\"}", "null", ""] {
        let (status, _) = call(Method::POST, "/api/watchlists", body).await;
        assert_eq!(status, 401, "body {body:?}");
    }
}

#[tokio::test]
async fn radar_and_delete_require_a_bearer_before_path_validation() {
    assert_eq!(call(Method::POST, "/api/watchlists/me", "").await.0, 401);
    let (status, _) = call(Method::DELETE, "/api/watchlists/not-a-uuid", "").await;
    assert_eq!(status, 401);
}

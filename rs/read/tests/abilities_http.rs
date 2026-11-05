//! Abilities are mounted with authentication, not the public read fallback.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config};

#[tokio::test]
async fn abilities_require_the_same_bearer_authentication_as_me() {
    let config: Config = serde_json::from_value(serde_json::json!({
        "environment":"test", "release_sha":verdaxis_read::compiled_sha(),
        "migration_revision":"ob_20261031_offboarding",
        "database_url":"postgres://rust_app:synthetic@127.0.0.1:9/abilities_test",
        "db_session_time_zone":"UTC", "bind":"127.0.0.1:0", "pool_max":3,
        "read_admission_slots":2, "python_budget":52, "configured_max_connections":100,
        "phase2":{"jwt_secret":"synthetic-test-key-at-least-32-characters",
        "jwt_issuer":"test", "jwt_audience":"test", "frontend_url":"https://app.example.test",
        "email_from":"team@example.test", "resend_api_key":"synthetic", "auth_domain_datasets":"null"}
    })).unwrap();
    let pool = |size| {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(size)
            .connect_lazy(&config.database_url)
            .unwrap()
    };
    let state = AppState::new(
        config.clone(),
        verdaxis_read::runtime::db::Pools {
            read: pool(2),
            health: pool(1),
        },
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap(),
    )
    .unwrap();
    let app = verdaxis_read::router(state);
    for path in ["/api/auth/me", "/api/auth/me/abilities"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
    }
}

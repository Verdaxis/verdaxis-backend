//! The serving composition exposes the credential area without business routes.
use axum::{body::Body, http::Request};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config, MarketPolicy, Phase2Options};

async fn app() -> axum::Router {
    let config = Config {
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "p3_20261020_auth_tokens".into(),
        database_url: "postgres://unused:unused@127.0.0.1:1/composition_test".into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        kyc_execution_gate_enabled: true,
        phase2: Phase2Options {
            jwt_secret: Some("composition-test-signing-authority-000000000".into()),
            jwt_issuer: Some("composition-api".into()),
            jwt_audience: Some("composition-web".into()),
            frontend_url: Some("https://staging.verdaxis.exchange".into()),
            email_from: Some("noreply@example.test".into()),
            resend_api_key: Some("disposable-mail-key".into()),
            ..Default::default()
        },
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new().max_connections(4)
            .connect_lazy(&config.database_url).unwrap(),
        health: sqlx::postgres::PgPoolOptions::new().max_connections(1)
            .connect_lazy(&config.database_url).unwrap(),
    };
    let state = AppState::new(config, pools, MarketPolicy {
        source_sha: verdaxis_read::compiled_sha().into(), products: vec![], points: vec![],
    }).unwrap();
    verdaxis_read::runtime::http::router_with(axum::Router::new(), state)
}

#[tokio::test]
async fn credential_operations_reach_their_admission_boundary() {
    let app = app().await;
    for (method, path, expected) in [
        ("POST", "/api/auth/login", 403),
        ("POST", "/api/auth/refresh", 403),
        ("POST", "/api/auth/logout", 403),
        ("PUT", "/api/auth/me/password", 403),
        ("POST", "/api/auth/invitations/accept", 403),
        ("GET", "/api/auth/stream-token", 401),
        ("POST", "/api/auth/forgot-password", 422),
        ("POST", "/api/auth/reset-password", 422),
        ("POST", "/api/auth/verify-email", 422),
        ("POST", "/api/auth/resend-verification-email", 422),
        ("POST", "/api/auth/register", 422),
        ("POST", "/api/auth/register-with-org", 422),
    ] {
        for suffix in ["", "/"] {
            let response = app.clone().oneshot(Request::builder().method(method)
                .uri(format!("{path}{suffix}"))
                .header("origin", "https://untrusted.example.test")
                .header("cookie", "refresh_token=invalid")
                .header("content-type", "application/json")
                .body(Body::from("{}" )).unwrap()).await.unwrap();
            assert_eq!(response.status().as_u16(), expected, "{method} {path}{suffix}");
        }
    }
    for path in ["/api/auth/me", "/api/auth/invitations/resolve", "/api/auth/survey"] {
        let response = app.clone().oneshot(Request::builder().method("POST").uri(path)
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }
}

#[tokio::test]
async fn ingress_and_credential_handlers_share_one_preauth_charge() {
    let app = app().await;
    for index in 0..31 {
        let response = app.clone().oneshot(Request::builder().method("POST")
            .uri("/api/auth/register").header("content-type", "application/json")
            .body(Body::from("{}")).unwrap()).await.unwrap();
        assert_eq!(response.status().as_u16(), if index < 30 { 422 } else { 429 });
    }
}

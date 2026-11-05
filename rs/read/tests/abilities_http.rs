//! Abilities are mounted with authentication, not the public read fallback.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config};

fn application(url: &str) -> axum::Router {
    let config: Config = serde_json::from_value(serde_json::json!({
        "environment":"test", "release_sha":verdaxis_read::compiled_sha(),
        "migration_revision":"ob_20261031_offboarding",
        "database_url":url,
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
    verdaxis_read::router(state)
}

#[tokio::test]
async fn abilities_require_the_same_bearer_authentication_as_me() {
    let app = application("postgres://rust_app:synthetic@127.0.0.1:9/abilities_test");
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

fn token(user: uuid::Uuid, expired: bool) -> String {
    let now = chrono::Utc::now().timestamp();
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &serde_json::json!({"sub":user,"type":"access","iat":now-600,
            "exp":if expired { now-300 } else { now+600 },"iss":"test","aud":"test"}),
        &jsonwebtoken::EncodingKey::from_secret(b"synthetic-test-key-at-least-32-characters"),
    )
    .unwrap()
}

async fn authenticated_reply(
    app: &axum::Router,
    path: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn expired_access_token_matches_profile_refusal_without_database() {
    let app = application("postgres://rust_app:synthetic@127.0.0.1:9/abilities_test");
    let bearer = token(uuid::Uuid::new_v4(), true);
    let profile = authenticated_reply(&app, "/api/auth/me", &bearer).await;
    assert_eq!(profile.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        authenticated_reply(&app, "/api/auth/me/abilities", &bearer).await,
        profile
    );
}

#[cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;

#[cfg(feature = "contract-tests")]
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL; run_abilities.sh"]
async fn revoked_cutoff_and_pending_accounts_match_profile_refusals() {
    use sqlx::Connection;
    let db = market_database::MarketDatabase::new("RUST_ABILITIES").await;
    let mut admin = sqlx::PgConnection::connect(&db.admin_url).await.unwrap();
    let user = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,role,status,email_verified) VALUES($1,'abilities-auth@example.test','unused','BUYER','APPROVED',true)")
        .bind(user).execute(&mut admin).await.unwrap();
    let app = application(&db.runtime_url);
    let bearer = token(user, false);
    sqlx::query("UPDATE users SET authentication_revoked_at=now() WHERE id=$1")
        .bind(user)
        .execute(&mut admin)
        .await
        .unwrap();
    let profile = authenticated_reply(&app, "/api/auth/me", &bearer).await;
    assert_eq!(profile.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        authenticated_reply(&app, "/api/auth/me/abilities", &bearer).await,
        profile
    );
    sqlx::query("UPDATE users SET authentication_revoked_at=NULL,status='PENDING' WHERE id=$1")
        .bind(user)
        .execute(&mut admin)
        .await
        .unwrap();
    let profile = authenticated_reply(&app, "/api/auth/me", &bearer).await;
    assert_eq!(profile.0, StatusCode::FORBIDDEN);
    assert_eq!(
        authenticated_reply(&app, "/api/auth/me/abilities", &bearer).await,
        profile
    );
}

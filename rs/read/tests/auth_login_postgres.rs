#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, auth, runtime};
#[allow(dead_code)]
#[path = "../src/auth/login.rs"]
mod login;

use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use chrono::Utc;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{limits::SharedLimits, password};

const PASSWORD: &str = "SyntheticPassword123";

async fn setup() -> (PgPool, PgPool, login::LoginState, Uuid) {
    let owner = PgPoolOptions::new().max_connections(3).connect(&std::env::var("AUTH_LOGIN_OWNER_URL").expect("owner URL")).await.unwrap();
    let pool = PgPoolOptions::new().max_connections(4).connect(&std::env::var("AUTH_LOGIN_DATABASE_URL").expect("runtime URL")).await.unwrap();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, password_hash, role, status, email_verified, must_change_password) VALUES ($1,$2,$3,'BUYER','APPROVED',true,false)")
        .bind(id).bind(format!("{id}@login.invalid")).bind(password::hash(PASSWORD).await.unwrap()).execute(&owner).await.unwrap();
    let state = login::LoginState::new(pool.clone(), "production", "synthetic-login-key-at-least-32-characters", None, "verdaxis", "verdaxis-api", SharedLimits::default()).await;
    (owner, pool, state, id)
}

async fn send(state: login::LoginState, id: Uuid, password: &str, device: &str) -> axum::response::Response {
    login::router(state).oneshot(Request::post("/api/auth/login")
        .header("origin", "https://app.verdaxis.exchange")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("device_session={device}"))
        .body(Body::from(format!("username={id}%40login.invalid&password={password}"))).unwrap()).await.unwrap()
}

fn device() -> String { verdaxis_read::auth::token_bytes::generate_token() }

async fn cleanup(owner: &PgPool, id: Uuid) {
    sqlx::query("DELETE FROM user_login_days WHERE user_id=$1").bind(id).execute(owner).await.unwrap();
    sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1").bind(id).execute(owner).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1").bind(id).execute(owner).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_success_returns_access_only_and_sets_both_cookies() {
    let (owner, _, state, id) = setup().await;
    let response = send(state, id, PASSWORD, &device()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookies: Vec<_> = response.headers().get_all("set-cookie").iter().map(|v| v.to_str().unwrap().to_owned()).collect();
    assert_eq!(cookies.len(), 2);
    for name in ["refresh_token=", "device_session="] {
        let cookie = cookies.iter().find(|c| c.starts_with(name)).unwrap();
        for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/api/auth", "Max-Age=604800"] { assert!(cookie.contains(attribute)); }
    }
    let body: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 2);
    assert_eq!(body["token_type"], "bearer");
    let settings = verdaxis_read::auth::VerificationSettings::new("synthetic-login-key-at-least-32-characters".into(), None, "verdaxis".into(), "verdaxis-api".into());
    assert_eq!(verdaxis_read::auth::verify_access(body["access_token"].as_str().unwrap(), &settings, Utc::now()).unwrap().subject, id);
    cleanup(&owner, id).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_credential_refusal_vectors() {
    let (owner, _, mut state, id) = setup().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    state.hooks.verifications = Some(calls.clone());
    let before: Option<chrono::DateTime<Utc>> = sqlx::query_scalar("SELECT last_login FROM users WHERE id=$1").bind(id).fetch_one(&owner).await.unwrap();
    let mut results = Vec::new();
    for subject in [id, Uuid::new_v4()] {
        calls.store(0, std::sync::atomic::Ordering::SeqCst);
        let response = send(state.clone(), subject, "WrongPassword123", &device()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        results.push((response.headers().clone(), to_bytes(response.into_body(), 4096).await.unwrap()));
    }
    assert_eq!(results[0], results[1]);
    let after: Option<chrono::DateTime<Utc>> = sqlx::query_scalar("SELECT last_login FROM users WHERE id=$1").bind(id).fetch_one(&owner).await.unwrap();
    assert_eq!(before, after);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1").bind(id).fetch_one(&owner).await.unwrap(), 0);
    cleanup(&owner, id).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_account_admission_vectors() {
    let (owner, _, state, id) = setup().await;
    for (status, verified, code) in [("APPROVED", false, "AUTH_EMAIL_UNVERIFIED"), ("PENDING", true, "AUTH_ACCOUNT_NOT_ACTIVE"), ("REJECTED", true, "AUTH_ACCOUNT_NOT_ACTIVE")] {
        sqlx::query("UPDATE users SET status=$2,email_verified=$3 WHERE id=$1").bind(id).bind(status).bind(verified).execute(&owner).await.unwrap();
        let response = send(state.clone(), id, PASSWORD, &device()).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(),4096).await.unwrap()).unwrap();
        assert_eq!(body["detail"]["code"], code);
    }
    cleanup(&owner, id).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_writes_one_family_per_device() {
    let (owner, _, state, id) = setup().await;
    let device = device();
    for _ in 0..2 { assert_eq!(send(state.clone(), id, PASSWORD, &device).await.status(), StatusCode::OK); }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(id).fetch_one(&owner).await.unwrap(), 1);
    let row = sqlx::query("SELECT login_count,first_login_at,last_login_at FROM user_login_days WHERE user_id=$1").bind(id).fetch_one(&owner).await.unwrap();
    assert_eq!(row.get::<i32,_>("login_count"), 2);
    assert!(row.get::<chrono::DateTime<Utc>,_>("last_login_at") > row.get::<chrono::DateTime<Utc>,_>("first_login_at"));
    cleanup(&owner, id).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_second_account_same_device_revokes_first() {
    let (owner, _, state, id) = setup().await;
    let (_, _, _, other) = setup().await;
    let device = device();
    for subject in [id, other] { assert_eq!(send(state.clone(), subject, PASSWORD, &device).await.status(), StatusCode::OK); }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(id).fetch_one(&owner).await.unwrap(), 0);
    cleanup(&owner, id).await;
    cleanup(&owner, other).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_session_failure_rolls_back_and_is_retryable() {
    let (owner, _, mut state, id) = setup().await;
    state.hooks.conflicts = Arc::new(std::sync::atomic::AtomicUsize::new(1));
    assert_eq!(send(state.clone(), id, PASSWORD, &device()).await.status(), StatusCode::OK);
    state.hooks.conflicts.store(2, std::sync::atomic::Ordering::SeqCst);
    let response = send(state.clone(), id, PASSWORD, &device()).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1").bind(id).fetch_one(&owner).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_, i32>("SELECT login_count FROM user_login_days WHERE user_id=$1").bind(id).fetch_one(&owner).await.unwrap(), 1);
    cleanup(&owner, id).await;
}

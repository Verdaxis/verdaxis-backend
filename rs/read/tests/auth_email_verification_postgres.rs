//! E3/E4 PostgreSQL component checks.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
use auth::token_bytes::token_digest;
use chrono::{Duration, Utc};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e3_consumes_and_assigns_once() {
    let pool = sqlx::PgPool::connect(
        &std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").expect("disposable database"),
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    let token = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users (id,email,password_hash,status,email_verified,must_change_password,email_verification_token_hash,email_verification_token_expires_at) VALUES ($1,$2,'unused','PENDING',false,false,$3,$4)")
        .bind(id).bind(format!("{id}@example.com")).bind(token_digest(&token)).bind(Utc::now()+Duration::hours(24)).execute(&pool).await.unwrap();
    let settings = email_verification::EmailSettings::new(
        pool.clone(),
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(id).execute(&mut *holder).await.unwrap();
    let waiting_settings = settings.clone();
    let waiting_token = token.clone();
    let waiting = tokio::spawn(async move { email_verification::verify(&waiting_settings, &waiting_token).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE cardinality(pg_blocking_pids(pid))>0 AND query LIKE '%email_verification_token_hash%')").fetch_one(&pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    holder.commit().await.unwrap();
    let result = waiting.await.unwrap().unwrap();
    assert_eq!(result["email"], format!("{id}@example.com"));
    assert_eq!(result["requires_org"], false);
    email_verification::resend(&settings,&format!("{id}@example.com"),true).await.unwrap();
    let row: (bool,Option<String>,Option<chrono::DateTime<Utc>>, Option<String>) = sqlx::query_as("SELECT email_verified,email_verification_token_hash,email_verification_token_expires_at,referral_code FROM users WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(row.0 && row.1.is_none() && row.2.is_none() && row.3.is_some());
    assert_eq!(
        email_verification::verify(&settings, &token)
            .await
            .unwrap_err()
            .status,
        axum::http::StatusCode::BAD_REQUEST
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e4_response_and_delivery_boundary() {
    use axum::{body::{Body, to_bytes}, http::Request, routing::post, Router};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use tower::ServiceExt;
    let pool = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let release_server = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_server = calls.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", post(move || {
            let release = release_server.clone();
            let calls = calls_server.clone();
            async move { calls.fetch_add(1, Ordering::SeqCst); release.notified().await; axum::http::StatusCode::OK }
        }))).await.unwrap();
    });
    let mail = email::MailDispatcher::new(crud::referrals::invite::MailClient::loopback(format!("http://{address}/"), "synthetic".into()).unwrap());
    let settings = email_verification::EmailSettings::new(pool.clone(), mail.clone());
    let mut expected = None;
    let mut ids = Vec::new();
    for verified in [Some(false), Some(true), None] {
        let id = Uuid::new_v4();
        let address = format!("{id}@example.com");
        if let Some(verified) = verified {
            sqlx::query("INSERT INTO users (id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',$3,false)").bind(id).bind(&address).bind(verified).execute(&pool).await.unwrap();
            ids.push(id);
        }
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), email_verification::router(settings.clone()).oneshot(Request::post("/api/auth/resend-verification-email").header("content-type","application/json").body(Body::from(serde_json::json!({"email":address}).to_string())).unwrap())).await.unwrap().unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(),4096).await.unwrap();
        let result = (status,headers,body);
        if let Some(expected) = &expected { assert_eq!(&result,expected); } else { expected=Some(result); }
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
    }).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),1);
    release.notify_waiters();
    mail.drain().await;
    for id in ids { sqlx::query("DELETE FROM users WHERE id=$1").bind(id).execute(&pool).await.unwrap(); }
    server.abort();
    let _ = server.await;
    pool.close().await;
}


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
    let result = email_verification::verify(&settings, &token).await.unwrap();
    assert_eq!(result["email"], format!("{id}@example.com"));
    assert_eq!(result["requires_org"], false);
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

//! E1/E2 PostgreSQL component checks.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
mod email;
#[path = "../src/auth/email_verification.rs"]
mod email_verification;
#[path = "../src/auth/password_reset.rs"]
mod password_reset;
use uuid::Uuid;
use chrono::{Duration,Utc};
use auth::token_bytes::token_digest;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e2_consumes_and_revokes() {
    let pool = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").expect("disposable database")).await.unwrap();
    let id = Uuid::new_v4();
    let address = format!("{id}@example.com");
    let token = Uuid::new_v4().to_string();
    let invitation = token_digest("invitation-fixture");
    sqlx::query("INSERT INTO users (id,email,password_hash,status,email_verified,must_change_password,password_reset_token_hash,password_reset_expires) VALUES ($1,$2,'unused','APPROVED',true,true,$3,$4)").bind(id).bind(&address).bind(&invitation).bind(Utc::now()+Duration::hours(1)).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO password_reset_tokens (user_id,token_hash,expires_at) VALUES ($1,$2,$3)").bind(id).bind(token_digest(&token)).bind(Utc::now()+Duration::hours(1)).execute(&pool).await.unwrap();
    let settings = email_verification::EmailSettings::new(pool.clone(), email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()));
    assert!(password_reset::reset(&settings,"invitation-fixture","New-password-123").await.is_err());
    password_reset::reset(&settings,&token,"New-password-123").await.unwrap();
    assert!(password_reset::reset(&settings,&token,"Another-password-123").await.is_err());
    let row:(String,bool,Option<chrono::DateTime<Utc>>,String) = sqlx::query_as("SELECT password_hash,must_change_password,password_changed_at,password_reset_token_hash FROM users WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(auth::password::verify("New-password-123",&row.0).await);
    assert!(!row.1 && row.2.is_some());
    assert_eq!(row.3,invitation);
    sqlx::query("DELETE FROM audit_logs WHERE user_id=$1").bind(id).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1").bind(id).execute(&pool).await.unwrap();
    pool.close().await;
}

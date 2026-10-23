//! E3/E4 credential operations. Runtime composition supplies shared quotas.
use axum::{Router, Json, extract::{State, Query, ConnectInfo}, http::{StatusCode, HeaderMap}, routing::post};
use std::net::SocketAddr;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use crate::{ApiError, ApiErrorKind};
use crate::auth::{error::{AuthCode, AuthFailure}, limits::SharedLimits, token_bytes::{generate_token, token_digest}};
use super::email::{AuthMail, MailDispatcher};

#[derive(Clone)]
pub struct EmailSettings {
    pub pool: PgPool,
    pub mail: MailDispatcher,
    pub limits: SharedLimits,
}
impl EmailSettings {
    pub fn new(pool: PgPool, mail: MailDispatcher) -> Self { Self { pool, mail, limits: SharedLimits::default() } }
}

pub fn database_error(_: sqlx::Error) -> ApiError {
    ApiError { status: StatusCode::SERVICE_UNAVAILABLE, detail: json!({"code":"DATABASE_UNAVAILABLE","message":"Database operation failed"}), retry_after: None, kind: ApiErrorKind::Expected }
}
fn invalid() -> ApiError { AuthFailure::new(AuthCode::EmailTokenInvalid).into() }

pub async fn verify(settings: &EmailSettings, token: &str) -> Result<Value, ApiError> {
    if token.is_empty() || token.len() > 1024 { return Err(invalid()); }
    let digest = token_digest(token);
    for attempt in 0..3 {
        match verify_attempt(settings, &digest).await {
            Err(error) if error.as_database_error().is_some_and(|error| matches!(error.code().as_deref(), Some("40001" | "40P01"))) && attempt < 2 => continue,
            Err(error) => return Err(database_error(error)),
            Ok(result) => return result,
        }
    }
    unreachable!("bounded verification attempts")
}
async fn verify_attempt(settings: &EmailSettings, digest: &str) -> Result<Result<Value, ApiError>, sqlx::Error> {
    let mut tx = settings.pool.begin().await?;
    let now = Utc::now();
    let user: Option<(uuid::Uuid, String, Option<String>)> = sqlx::query_as("SELECT id,email,referral_code FROM users WHERE email_verification_token_hash=$1 AND email_verification_token_expires_at > $2 AND NOT email_verified FOR UPDATE")
        .bind(digest).bind(now).fetch_optional(&mut *tx).await?;
    if let Some((id,email,code)) = user {
        sqlx::query("UPDATE users SET email_verified=true,email_verification_token_hash=NULL,email_verification_token_expires_at=NULL WHERE id=$1").bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE referrals SET status='VERIFIED',verified_at=$2 WHERE referred_user_id=$1 AND status='SIGNED_UP'").bind(id).bind(now).execute(&mut *tx).await?;
        if code.is_none_or(|code| code.is_empty()) {
            // A savepoint keeps a unique-code collision local to this probe.
            let mut assigned = false;
            for candidate in crate::crud::referrals::Candidates::new().take(10) {
                sqlx::query("SAVEPOINT verification_code").execute(&mut *tx).await?;
                let result = sqlx::query("UPDATE users SET referral_code=$2 WHERE id=$1").bind(id).bind(candidate).execute(&mut *tx).await;
                match result {
                    Ok(_) => { sqlx::query("RELEASE SAVEPOINT verification_code").execute(&mut *tx).await?; assigned=true; break; }
                    Err(error) if error.as_database_error().is_some_and(|error| error.is_unique_violation()) => {
                        sqlx::query("ROLLBACK TO SAVEPOINT verification_code").execute(&mut *tx).await?;
                        sqlx::query("RELEASE SAVEPOINT verification_code").execute(&mut *tx).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            if !assigned { return Ok(Err(database_error(sqlx::Error::RowNotFound))); }
        }
        tx.commit().await?;
        return Ok(Ok(json!({"message":"Email verified","email":email,"requires_org":false})));
    }
    // Pending links retain their opaque registration credential for E6.
    let pending: Option<(String,)> = sqlx::query_as("SELECT email FROM pending_registrations WHERE token_hash=$1 AND used_at IS NULL AND expires_at>$2 FOR UPDATE")
        .bind(digest).bind(now).fetch_optional(&mut *tx).await?;
    tx.rollback().await?;
    Ok(match pending { Some((email,)) => Ok(json!({"message":"Continue registration","email":email,"requires_org":true})), None => Err(invalid()) })
}

pub async fn resend(settings: &EmailSettings, address: &str, monitor_admitted: bool) -> Result<Value, ApiError> {
    let mut tx = settings.pool.begin().await.map_err(database_error)?;
    let user: Option<(uuid::Uuid, Option<String>)> = sqlx::query_as("SELECT id,first_name FROM users WHERE email=$1 AND NOT email_verified FOR UPDATE")
        .bind(address).fetch_optional(&mut *tx).await.map_err(database_error)?;
    let mail = if let Some((id,name)) = user {
        let token = generate_token();
        sqlx::query("UPDATE users SET email_verification_token_hash=$2,email_verification_token_expires_at=$3 WHERE id=$1")
            .bind(id).bind(token_digest(&token)).bind(Utc::now()+Duration::hours(24)).execute(&mut *tx).await.map_err(database_error)?;
        Some(AuthMail::Verification { to: address.into(), name: name.unwrap_or_else(|| "there".into()), token, pending: None })
    } else { None };
    tx.commit().await.map_err(database_error)?;
    if !super::email::suppress_verification(monitor_admitted, address) && let Some(mail) = mail { settings.mail.dispatch(mail); }
    Ok(json!({"message":"If applicable, a verification link will be sent."}))
}

#[derive(Deserialize)]
struct VerifyQuery { token: String }
pub fn router(settings: EmailSettings) -> Router {
    Router::new().route("/api/auth/verify-email", post(verify_route)).route("/api/auth/resend-verification-email", post(resend_route)).with_state(settings)
}
async fn verify_route(State(settings): State<EmailSettings>, peer: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap, Query(query): Query<VerifyQuery>) -> Result<Json<Value>, ApiError> {
    let client = client_address(peer, &headers);
    settings.limits.check_endpoint(&client, "/api/auth/verify-email", Utc::now().timestamp() as u64)?;
    let mut result = verify(&settings, &query.token).await?;
    if result["requires_org"] == true { result["registration_token"] = query.token.into(); }
    Ok(Json(result))
}
async fn resend_route(State(settings): State<EmailSettings>, peer: Option<ConnectInfo<SocketAddr>>, headers: HeaderMap, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let address = crate::crud::referrals::invite::normalize_email(&body)?;
    let client = client_address(peer, &headers);
    settings.limits.check_endpoint(&client, "/api/auth/resend-verification-email", Utc::now().timestamp() as u64)?;
    resend(&settings, &address.to_lowercase(), false).await.map(Json)
}

pub fn client_address(peer: Option<ConnectInfo<SocketAddr>>, headers: &HeaderMap) -> String {
    crate::runtime::http::client_ip(peer.map(|peer| peer.0.ip()), headers.get("x-forwarded-for").and_then(|value| value.to_str().ok()))
}

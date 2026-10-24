//! E1/E2 operations over the dedicated reset-token store.
use super::email::AuthMail;
use super::email_verification::{
    EmailSettings, auth_json, client_address, database_error, email_address, retry_transaction,
};
use crate::ApiError;
use crate::auth::{
    error::{AuthCode, AuthFailure},
    password, store,
    token_bytes::{generate_token, token_digest},
};
use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;

fn invalid() -> ApiError {
    AuthFailure::new(AuthCode::ResetTokenInvalid).into()
}
fn validation() -> ApiError {
    AuthFailure::new(AuthCode::ValidationFailed).into()
}
pub async fn forgot(settings: &EmailSettings, address: &str) -> Result<Value, ApiError> {
    store::sweep_expired_reset_tokens(&settings.pool, Utc::now())
        .await
        .map_err(database_error)?;
    let mail = retry_transaction(|| forgot_attempt(settings, address)).await?;
    if let Some(mail) = mail {
        settings.mail.dispatch(mail);
    }
    Ok(json!({"message":"If applicable, a password reset link will be sent."}))
}
async fn forgot_attempt(
    settings: &EmailSettings,
    address: &str,
) -> Result<Option<AuthMail>, sqlx::Error> {
    let mut tx = settings.pool.begin().await?;
    let user = store::lock_user_by_email(&mut tx, address).await?;
    let mail = if let Some(user) = user.filter(|user| user.email_verified) {
        let now = Utc::now();
        let token = generate_token();
        store::upsert_reset_token(
            &mut tx,
            &store::NewResetToken {
                user_id: user.id,
                token_hash: token_digest(&token),
                expires_at: now + Duration::hours(1),
            },
            now,
        )
        .await?;
        audit(&mut tx, user.id, "PASSWORD_RESET_REQUESTED").await?;
        Some(AuthMail::Reset {
            to: address.into(),
            name: "there".into(),
            token,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(mail)
}

pub async fn reset(
    settings: &EmailSettings,
    token: &str,
    new_password: &str,
) -> Result<Value, ApiError> {
    password::validate_bytes(new_password).map_err(|_| validation())?;
    if token.is_empty() || token.len() > 1024 {
        return Err(invalid());
    }
    let digest = token_digest(token);
    let hash = password::hash(new_password)
        .await
        .map_err(|_| validation())?;
    for attempt in 0..3 {
        match reset_attempt(settings, &digest, &hash).await {
            Err(error)
                if error.as_database_error().is_some_and(|error| {
                    matches!(error.code().as_deref(), Some("40001" | "40P01"))
                }) && attempt < 2 =>
            {
                continue;
            }
            Err(error) => return Err(database_error(error)),
            Ok(result) => return result,
        }
    }
    unreachable!("bounded reset attempts")
}
async fn reset_attempt(
    settings: &EmailSettings,
    digest: &str,
    hash: &str,
) -> Result<Result<Value, ApiError>, sqlx::Error> {
    let mut tx = settings.pool.begin().await?;
    let Some(id) = store::reset_token_owner(&mut tx, digest).await? else {
        return Ok(Err(invalid()));
    };
    let Some(user) = store::lock_user(&mut tx, id).await? else {
        return Ok(Err(invalid()));
    };
    let now = Utc::now();
    if !store::consume_reset_token(&mut tx, id, digest, now).await? {
        return Ok(Err(invalid()));
    }
    let cutoff = user
        .password_changed_at
        .map_or(now, |old| now.max(old + Duration::microseconds(1)));
    sqlx::query("UPDATE users SET password_hash=$2,password_changed_at=$3,must_change_password=false WHERE id=$1").bind(id).bind(hash).bind(cutoff).execute(&mut *tx).await?;
    store::revoke_user_families(&mut tx, id).await?;
    audit(&mut tx, id, "PASSWORD_RESET_COMPLETED").await?;
    tx.commit().await?;
    Ok(Ok(
        json!({"message":"Password updated. Sign in with your new password."}),
    ))
}
async fn audit(
    conn: &mut sqlx::PgConnection,
    id: uuid::Uuid,
    action: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_logs (id,user_id,action,resource_type,resource_id) VALUES ($1,$2,$3,'user',$4)").bind(uuid::Uuid::new_v4()).bind(id).bind(action).bind(id.to_string()).execute(conn).await?;
    Ok(())
}
#[derive(Deserialize)]
struct ResetRequest {
    token: String,
    new_password: String,
}
pub fn router(settings: EmailSettings) -> Router {
    Router::new()
        .route("/api/auth/forgot-password", post(forgot_route))
        .route("/api/auth/reset-password", post(reset_route))
        .with_state(settings)
}
async fn forgot_route(
    State(settings): State<EmailSettings>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<Value>, ApiError> {
    let client = client_address(peer, &headers);
    let now = Utc::now().timestamp() as u64;
    settings
        .limits
        .preauth_check(&client, "/api/auth/forgot-password", now)
        .map_err(|error| ApiError::from(error.auth_failure()))?;
    let body: Value = auth_json(&headers, body).await?;
    let address = email_address(&body)?;
    settings
        .limits
        .check_endpoint(&client, "/api/auth/forgot-password", now)?;
    forgot(&settings, &address.to_lowercase()).await.map(Json)
}
async fn reset_route(
    State(settings): State<EmailSettings>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<Value>, ApiError> {
    let body: ResetRequest = auth_json(&headers, body).await?;
    password::validate_bytes(&body.new_password).map_err(|_| validation())?;
    settings.limits.check_endpoint(
        &client_address(peer, &headers),
        "/api/auth/reset-password",
        Utc::now().timestamp() as u64,
    )?;
    reset(&settings, &body.token, &body.new_password)
        .await
        .map(Json)
}

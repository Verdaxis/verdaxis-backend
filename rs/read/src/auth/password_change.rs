//! Bearer-authorized password replacement in one device-ordered transaction.
use crate::{
    ApiError,
    auth::{
        AccountState, AuthError, TokenIssuer, VerificationSettings, VerifiedAccess,
        cookies::AuthCookies,
        error::{AuthCode, AuthFailure, read_auth_body},
        limits::SharedLimits,
        lock::WriteConflict,
        origin::require_trusted_origin,
        password,
        store::{self, NewRefreshRow},
        token_bytes::{generate_token, token_digest},
        validate_account, verify_authorization,
    },
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::put,
};
use axum_extra::extract::CookieJar;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;
#[derive(Clone)]
pub struct PasswordChangeState {
    pool: PgPool,
    environment: String,
    issuer: Arc<TokenIssuer>,
    verification: Arc<VerificationSettings>,
    pub limits: SharedLimits,
}
impl PasswordChangeState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        environment: &str,
        secret: &str,
        previous: Option<&str>,
        issuer: &str,
        audience: &str,
        limits: SharedLimits,
    ) -> Self {
        Self {
            pool,
            environment: environment.into(),
            issuer: Arc::new(TokenIssuer::new(secret, issuer, audience)),
            verification: Arc::new(VerificationSettings::new(
                secret.into(),
                previous.map(str::to_owned),
                issuer.into(),
                audience.into(),
            )),
            limits,
        }
    }
}
pub fn router(state: PasswordChangeState) -> Router {
    Router::new()
        .route("/api/auth/me/password", put(change_password))
        .route("/api/auth/me/password/", put(change_password))
        .with_state(state)
}
#[derive(Deserialize)]
struct Change {
    current_password: String,
    new_password: String,
}
async fn change_password(
    State(state): State<PasswordChangeState>,
    jar: CookieJar,
    request: Request,
) -> Response {
    match handle(&state, jar, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}
async fn handle(
    state: &PasswordChangeState,
    jar: CookieJar,
    request: Request,
) -> Result<Response, ApiError> {
    let presented = AuthCookies::read(&jar);
    require_trusted_origin(
        request.headers(),
        &state.environment,
        presented.cookie_authenticated(),
    )
    .map_err(|_| ApiError::from(AuthFailure::new(AuthCode::BrowserOriginRejected)))?;
    let access = verify_authorization(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        &state.verification,
        Utc::now(),
    )
    .map_err(auth_error)?;
    let client = crate::runtime::http::client_ip(
        request
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|p| p.0.ip()),
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok()),
    );
    state.limits.check_endpoint(
        &client,
        "/api/auth/me/password",
        Utc::now().timestamp().max(0) as u64,
    )?;
    let (parts, body) = request.into_parts();
    let body = read_auth_body(&parts.headers, body)
        .await
        .map_err(ApiError::from)?;
    let change: Change = serde_json::from_slice(&body)
        .map_err(|_| ApiError::from(AuthFailure::new(AuthCode::ValidationFailed)))?;
    password::validate_bytes(&change.new_password)
        .map_err(|_| ApiError::from(AuthFailure::new(AuthCode::ValidationFailed)))?;
    let device = presented
        .valid_device()
        .map(str::to_owned)
        .unwrap_or_else(generate_token);
    let device_hash = token_digest(&device);
    for attempt in 0..2 {
        match replace(state, &access, &change, &device_hash).await {
            Ok((access, refresh)) => {
                return Ok((
                    AuthCookies::for_environment(&state.environment).set(jar, &refresh, &device),
                    Json(json!({"access_token":access})),
                )
                    .into_response());
            }
            Err(ChangeError::Refused(error)) => return Err(error),
            Err(ChangeError::Database(error)) if attempt == 0 && error.is_write_conflict() => {
                continue;
            }
            Err(ChangeError::Database(_)) => {
                return Err(ApiError {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    ..ApiError::database()
                });
            }
        }
    }
    Err(ApiError::database())
}
enum ChangeError {
    Refused(ApiError),
    Database(sqlx::Error),
}
impl From<sqlx::Error> for ChangeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
fn auth_error(error: AuthError) -> ApiError {
    ApiError {
        status: StatusCode::from_u16(error.status()).unwrap(),
        detail: error.body()["detail"].clone(),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}
async fn replace(
    state: &PasswordChangeState,
    access: &VerifiedAccess,
    change: &Change,
    device_hash: &str,
) -> Result<(String, String), ChangeError> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION READ WRITE")
        .execute(&mut *tx)
        .await?;
    store::lock_device(&mut tx, device_hash).await?;
    let user = store::lock_user(&mut tx, access.subject)
        .await?
        .ok_or_else(|| {
            ChangeError::Refused(AuthFailure::new(AuthCode::CredentialsInvalid).into())
        })?;
    // This endpoint is the explicit forced-password-change exception.
    let account = AccountState {
        exists: true,
        status: user.status.clone(),
        must_change_password: false,
        password_changed_at: user.password_changed_at.map(|v| v.timestamp_micros()),
        authentication_revoked_at: user.authentication_revoked_at.map(|v| v.timestamp_micros()),
    };
    validate_account(access, &account).map_err(|error| ChangeError::Refused(auth_error(error)))?;
    if !password::verify(&change.current_password, &user.password_hash).await {
        return Err(ChangeError::Refused(
            AuthFailure::new(AuthCode::CurrentPasswordInvalid).into(),
        ));
    }
    let hash = password::hash(&change.new_password)
        .await
        .map_err(|_| ChangeError::Refused(AuthFailure::new(AuthCode::ValidationFailed).into()))?;
    let now = Utc::now();
    let cutoff = store::revoke_user_authentication(
        &mut tx,
        user.id,
        now.max(user.password_changed_at.unwrap_or(now)),
    )
    .await?;
    sqlx::query("UPDATE users SET password_hash=$2,password_changed_at=$3,must_change_password=false WHERE id=$1").bind(user.id).bind(hash).bind(cutoff).execute(&mut *tx).await?;
    let family = Uuid::new_v4();
    let jti = generate_token();
    let access = state.issuer.access(user.id, now, Some(cutoff));
    let refresh = state
        .issuer
        .refresh(user.id, family, &jti, now, Some(cutoff));
    store::insert_refresh(
        &mut tx,
        &NewRefreshRow {
            user_id: user.id,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: device_hash.into(),
            expires_at: cutoff.max(now) + Duration::microseconds(1) + Duration::days(7),
        },
    )
    .await?;
    sqlx::query("INSERT INTO audit_logs (id,user_id,action,resource_type,resource_id,changes,timestamp) VALUES ($1,$2,'user.password_changed','user',$3,$4,$5)").bind(Uuid::new_v4()).bind(user.id).bind(user.id.to_string()).bind(json!({"password_changed":true})).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((access, refresh))
}

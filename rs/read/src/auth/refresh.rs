//! Cookie-bound refresh rotation.
use crate::{
    ApiError,
    auth::{
        AccountState, AuthError, TokenIssuance, TokenIssuer,
        cookies::AuthCookies,
        cutoff_refusal,
        error::{AuthCode, AuthFailure},
        limits::SharedLimits,
        lock::WriteConflict,
        origin::require_trusted_origin,
        read_issuance, read_subject,
        store::{self, NewRefreshRow},
        token_bytes::{generate_token, token_digest},
    },
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Request, State},
    response::{IntoResponse, Response},
    routing::post,
};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, errors::ErrorKind};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct RefreshState {
    pool: PgPool,
    environment: String,
    issuer: Arc<TokenIssuer>,
    keys: Arc<Vec<DecodingKey>>,
    jwt_issuer: String,
    audience: String,
    limits: SharedLimits,
    #[cfg(feature = "contract-tests")]
    pub hooks: RefreshHooks,
}
#[cfg(feature = "contract-tests")]
#[derive(Clone, Default)]
pub struct RefreshHooks {
    pub before_device: Option<Arc<tokio::sync::Barrier>>,
    pub user_locked: Option<Arc<tokio::sync::Barrier>>,
}
impl RefreshState {
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
            keys: Arc::new(
                std::iter::once(secret)
                    .chain(previous)
                    .map(|s| DecodingKey::from_secret(s.as_bytes()))
                    .collect(),
            ),
            jwt_issuer: issuer.into(),
            audience: audience.into(),
            limits,
            #[cfg(feature = "contract-tests")]
            hooks: RefreshHooks::default(),
        }
    }
}
pub fn router(state: RefreshState) -> Router {
    Router::new()
        .route("/api/auth/refresh", post(refresh))
        .route("/api/auth/refresh/", post(refresh))
        .with_state(state)
}
struct PresentedToken {
    subject: Uuid,
    family: Uuid,
    jti: String,
    issuance: TokenIssuance,
}
fn verify(state: &RefreshState, token: &str) -> Result<PresentedToken, AuthFailure> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[&state.jwt_issuer]);
    validation.set_audience(&[&state.audience]);
    validation.leeway = 0;
    let mut expired = false;
    let claims = state
        .keys
        .iter()
        .find_map(|key| match decode::<Value>(token, key, &validation) {
            Ok(data) => Some(data.claims),
            Err(error) => {
                expired |= matches!(error.kind(), ErrorKind::ExpiredSignature);
                None
            }
        })
        .ok_or_else(|| {
            AuthFailure::new(if expired {
                AuthCode::RefreshTokenExpired
            } else {
                AuthCode::RefreshTokenInvalid
            })
        })?;
    let invalid = || AuthFailure::new(AuthCode::RefreshTokenInvalid);
    if claims.get("type").and_then(Value::as_str) != Some("refresh") {
        return Err(invalid());
    }
    let subject = read_subject(&claims).map_err(|_| invalid())?;
    let family = claims
        .get("family_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(invalid)?;
    let jti = claims
        .get("jti")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)?
        .to_owned();
    Ok(PresentedToken {
        subject,
        family,
        jti,
        issuance: read_issuance(&claims),
    })
}
async fn refresh(State(state): State<RefreshState>, jar: CookieJar, request: Request) -> Response {
    match handle(&state, jar, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}
async fn handle(
    state: &RefreshState,
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
        "/api/auth/refresh",
        Utc::now().timestamp().max(0) as u64,
    )?;
    let token = verify(
        state,
        presented
            .refresh
            .as_deref()
            .ok_or_else(|| ApiError::from(AuthFailure::new(AuthCode::RefreshTokenMissing)))?,
    )
    .map_err(ApiError::from)?;
    let device = presented
        .valid_device()
        .ok_or_else(|| ApiError::from(AuthFailure::new(AuthCode::RefreshDeviceRequired)))?;
    for attempt in 0..2 {
        match rotate(state, &token, &token_digest(device)).await {
            Ok(Outcome::Issued(access, refresh)) => {
                return Ok((
                    AuthCookies::for_environment(&state.environment).set(jar, &refresh, device),
                    Json(json!({"access_token":access,"token_type":"bearer"})),
                )
                    .into_response());
            }
            Ok(Outcome::Refused(code)) => return Err(AuthFailure::new(code).into()),
            Ok(Outcome::Grace) => {
                return Err(AuthFailure::new(AuthCode::RefreshRotationInProgress)
                    .with_retry_after(1)
                    .into());
            }
            Err(error) if attempt == 0 && error.is_write_conflict() => continue,
            Err(_) => return Err(unavailable()),
        }
    }
    Err(unavailable())
}
fn unavailable() -> ApiError {
    ApiError {
        status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        ..ApiError::database()
    }
}
enum Outcome {
    Issued(String, String),
    Refused(AuthCode),
    Grace,
}
async fn rotate(
    state: &RefreshState,
    token: &PresentedToken,
    device_hash: &str,
) -> Result<Outcome, sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION READ WRITE")
        .execute(&mut *tx)
        .await?;
    #[cfg(feature = "contract-tests")]
    if let Some(barrier) = &state.hooks.before_device {
        barrier.wait().await;
    }
    store::lock_device(&mut tx, device_hash).await?;
    let Some(user) = store::lock_user(&mut tx, token.subject).await? else {
        return Ok(Outcome::Refused(AuthCode::RefreshSessionRevoked));
    };
    #[cfg(feature = "contract-tests")]
    if let Some(barrier) = &state.hooks.user_locked {
        barrier.wait().await;
        barrier.wait().await;
    }
    let Some(row) = store::lock_refresh_by_jti(&mut tx, &token_digest(&token.jti)).await? else {
        return Ok(Outcome::Refused(AuthCode::RefreshSessionRevoked));
    };
    if row.user_id != token.subject
        || row.family_id != token.family
        || row.device_id_hash.as_deref() != Some(device_hash)
    {
        return Ok(Outcome::Refused(AuthCode::RefreshSessionRevoked));
    }
    let account = AccountState {
        exists: true,
        status: user.status.clone(),
        must_change_password: user.must_change_password,
        password_changed_at: user.password_changed_at.map(|t| t.timestamp_micros()),
        authentication_revoked_at: user.authentication_revoked_at.map(|t| t.timestamp_micros()),
    };
    if let Some(error) = cutoff_refusal(token.issuance, &account) {
        return Ok(Outcome::Refused(
            if matches!(error, AuthError::PasswordChanged) {
                AuthCode::RefreshPasswordChanged
            } else {
                AuthCode::RefreshSessionRevoked
            },
        ));
    }
    if user.status != "APPROVED" {
        return Ok(Outcome::Refused(AuthCode::RefreshAccountInactive));
    }
    let now = Utc::now();
    if let Some(next) = row.replaced_by_jti_hash {
        let live = store::lock_refresh_by_jti(&mut tx, &next)
            .await?
            .is_some_and(|s| {
                s.user_id == row.user_id
                    && s.family_id == row.family_id
                    && s.device_id_hash.as_deref() == Some(device_hash)
                    && !s.revoked
                    && s.expires_at > now
            });
        if live && row.rotation_grace_until.is_some_and(|t| t >= now) {
            return Ok(Outcome::Grace);
        }
        revoke_family(&mut tx, row.family_id).await?;
        tx.commit().await?;
        return Ok(Outcome::Refused(AuthCode::RefreshTokenReplayed));
    }
    if row.revoked || row.expires_at <= now {
        return Ok(Outcome::Refused(AuthCode::RefreshSessionRevoked));
    }
    let jti = generate_token();
    let digest = token_digest(&jti);
    mark_rotated(&mut tx, &row.jti_hash, &digest, now).await?;
    store::insert_refresh(
        &mut tx,
        &NewRefreshRow {
            user_id: user.id,
            family_id: row.family_id,
            jti_hash: digest,
            device_id_hash: device_hash.into(),
            expires_at: now + Duration::days(7) + Duration::microseconds(1),
        },
    )
    .await?;
    let access = state.issuer.access(user.id, now, Some(now));
    let refresh = state
        .issuer
        .refresh(user.id, row.family_id, &jti, now, Some(now));
    tx.commit().await?;
    Ok(Outcome::Issued(access, refresh))
}
async fn revoke_family(conn: &mut sqlx::PgConnection, family: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE refresh_sessions SET revoked=true WHERE family_id=$1")
        .bind(family)
        .execute(conn)
        .await?;
    Ok(())
}
async fn mark_rotated(
    conn: &mut sqlx::PgConnection,
    jti: &str,
    next: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE refresh_sessions SET revoked=true,replaced_by_jti_hash=$2,rotation_grace_until=$3,last_used_at=$4 WHERE jti_hash=$1").bind(jti).bind(next).bind(now+Duration::seconds(5)).bind(now).execute(conn).await?;
    Ok(())
}

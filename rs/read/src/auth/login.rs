//! Form login and transaction-owned session issuance.
use std::sync::Arc;

use axum::{
    Form, Json, Router,
    extract::{ConnectInfo, FromRequest, Request, State},
    response::{IntoResponse, Response},
    routing::post,
};
use axum_extra::extract::CookieJar;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(feature = "contract-tests")]
use sqlx::PgConnection;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    ApiError,
    auth::{
        cookies::AuthCookies,
        error::{AuthCode, AuthFailure, read_auth_body},
        issue::{REFRESH_TOKEN_DAYS, TokenIssuer},
        limits::SharedLimits,
        lock::WriteConflict,
        origin::require_trusted_origin,
        password,
        store::{self, NewRefreshRow},
        token_bytes::{generate_token, token_digest},
    },
};

#[derive(Clone)]
pub struct LoginState {
    pool: PgPool,
    environment: String,
    issuer: Arc<TokenIssuer>,
    refresh_keys: Arc<Vec<DecodingKey>>,
    token_issuer: String,
    audience: String,
    dummy_hash: &'static str,
    pub limits: SharedLimits,
    #[cfg(feature = "contract-tests")]
    pub hooks: LoginHooks,
}

/// Component state; initialization completes password-worker preparation.
impl LoginState {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        pool: PgPool,
        environment: &str,
        current_secret: &str,
        previous_secret: Option<&str>,
        issuer: &str,
        audience: &str,
        limits: SharedLimits,
    ) -> Self {
        Self {
            pool,
            environment: environment.into(),
            issuer: Arc::new(TokenIssuer::new(current_secret, issuer, audience)),
            refresh_keys: Arc::new(
                std::iter::once(current_secret)
                    .chain(previous_secret)
                    .map(|s| DecodingKey::from_secret(s.as_bytes()))
                    .collect(),
            ),
            token_issuer: issuer.into(),
            audience: audience.into(),
            dummy_hash: password::dummy_hash().await,
            limits,
            #[cfg(feature = "contract-tests")]
            hooks: LoginHooks::default(),
        }
    }
}

#[cfg(feature = "contract-tests")]
#[derive(Clone, Default)]
pub struct LoginHooks {
    pub verifications: Option<Arc<std::sync::atomic::AtomicUsize>>,
    pub conflicts: Arc<std::sync::atomic::AtomicUsize>,
    /// Rendezvous after the user lock; a second rendezvous releases the attempt.
    pub user_locked: Option<Arc<tokio::sync::Barrier>>,
}

pub fn router(state: LoginState) -> Router {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/login/", post(login))
        .with_state(state)
}

/// Installed by the shared ingress after its pre-auth admission.
#[derive(Clone, Copy)]
pub struct PreauthCharged(pub u64);

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

async fn login(State(state): State<LoginState>, jar: CookieJar, request: Request) -> Response {
    match handle(&state, jar, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle(
    state: &LoginState,
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
    let peer = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|p| p.0.ip());
    let client = crate::runtime::http::client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok()),
    );
    let now = if let Some(charged) = request.extensions().get::<PreauthCharged>() {
        charged.0
    } else {
        let now = Utc::now().timestamp().max(0) as u64;
        state
            .limits
            .preauth_check(&client, request.uri().path(), now)
            .map_err(|r| ApiError::from(r.auth_failure()))?;
        now
    };
    state
        .limits
        .check_endpoint(&client, "/api/auth/login", now)?;
    let (parts, body) = request.into_parts();
    let body = read_auth_body(&parts.headers, body)
        .await
        .map_err(ApiError::from)?;
    let request = Request::from_parts(parts, axum::body::Body::from(body));
    let Form(credentials) = Form::<Credentials>::from_request(request, &())
        .await
        .map_err(|_| ApiError::from(AuthFailure::new(AuthCode::ValidationFailed)))?;
    if credentials.password.len() > password::MAX_PASSWORD_BYTES {
        return Err(AuthFailure::new(AuthCode::ValidationFailed).into());
    }
    let email = credentials.username.trim().to_lowercase();
    let device = presented
        .valid_device()
        .map(str::to_owned)
        .unwrap_or_else(generate_token);
    let device_hash = token_digest(&device);
    let family = presented
        .refresh
        .as_deref()
        .and_then(|token| presented_family(state, token));
    for attempt in 0..2 {
        match issue_attempt(state, &email, &credentials.password, &device_hash, family).await {
            Ok((access, refresh)) => {
                let jar =
                    AuthCookies::for_environment(&state.environment).set(jar, &refresh, &device);
                return Ok((
                    jar,
                    Json(json!({"access_token": access, "token_type": "bearer"})),
                )
                    .into_response());
            }
            Err(AttemptError::Refused(failure)) => return Err(failure.into()),
            Err(AttemptError::Database(error)) if attempt == 0 && error.is_write_conflict() => {
                continue;
            }
            Err(AttemptError::Database(_)) => return Err(database_unavailable()),
        }
    }
    Err(database_unavailable())
}

fn database_unavailable() -> ApiError {
    ApiError {
        status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        ..ApiError::database()
    }
}

enum AttemptError {
    Refused(AuthFailure),
    Database(sqlx::Error),
}
impl From<sqlx::Error> for AttemptError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

async fn issue_attempt(
    state: &LoginState,
    email: &str,
    password: &str,
    device_hash: &str,
    presented_family: Option<Uuid>,
) -> Result<(String, String), AttemptError> {
    let mut transaction = state.pool.begin().await?;
    // The serving pool defaults to read-only; writes opt in per transaction.
    sqlx::query("SET TRANSACTION READ WRITE")
        .execute(&mut *transaction)
        .await?;
    store::lock_device(&mut transaction, device_hash).await?;
    let user = store::lock_user_by_email(&mut transaction, email).await?;
    #[cfg(feature = "contract-tests")]
    if let Some(barrier) = &state.hooks.user_locked {
        barrier.wait().await;
        barrier.wait().await;
    }
    #[cfg(feature = "contract-tests")]
    if let Some(calls) = &state.hooks.verifications {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    let verified = password::verify(
        password,
        user.as_ref()
            .map(|u| u.password_hash.as_str())
            .unwrap_or(state.dummy_hash),
    )
    .await;
    let user = user
        .filter(|_| verified)
        .ok_or_else(|| AttemptError::Refused(AuthFailure::new(AuthCode::CredentialsInvalid)))?;
    if !user.email_verified {
        return Err(AttemptError::Refused(AuthFailure::new(
            AuthCode::EmailUnverified,
        )));
    }
    if user.status != "APPROVED" {
        return Err(AttemptError::Refused(AuthFailure::new(
            AuthCode::AccountNotActive,
        )));
    }
    let now = Utc::now();
    sqlx::query("UPDATE users SET last_login=$2 WHERE id=$1")
        .bind(user.id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO user_login_days (id,activity_date,user_id,organization_id,role,login_count,first_login_at,last_login_at) VALUES ($1,$2,$3,$4,$5,1,$6,$6) ON CONFLICT (activity_date,user_id) DO UPDATE SET login_count=user_login_days.login_count+1,first_login_at=LEAST(user_login_days.first_login_at,EXCLUDED.first_login_at),last_login_at=GREATEST(user_login_days.last_login_at,EXCLUDED.last_login_at)")
        .bind(Uuid::new_v4()).bind(now.date_naive()).bind(user.id).bind(user.organization_id).bind(&user.role).bind(now).execute(&mut *transaction).await?;
    store::revoke_device_families(&mut transaction, device_hash, presented_family).await?;
    let cutoff = user.password_changed_at.max(user.authentication_revoked_at);
    let family = Uuid::new_v4();
    let jti = generate_token();
    let access = state.issuer.access(user.id, now, cutoff);
    let refresh = state.issuer.refresh(user.id, family, &jti, now, cutoff);
    let issued = cutoff
        .filter(|t| *t >= now)
        .map(|t| t + Duration::microseconds(1))
        .unwrap_or(now);
    store::insert_refresh(
        &mut transaction,
        &NewRefreshRow {
            user_id: user.id,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: device_hash.to_owned(),
            expires_at: issued + Duration::days(REFRESH_TOKEN_DAYS),
        },
    )
    .await?;
    #[cfg(feature = "contract-tests")]
    inject_conflict(&mut transaction, &state.hooks).await?;
    transaction.commit().await?;
    Ok((access, refresh))
}

#[cfg(feature = "contract-tests")]
async fn inject_conflict(
    connection: &mut PgConnection,
    hooks: &LoginHooks,
) -> Result<(), sqlx::Error> {
    use std::sync::atomic::Ordering;
    if hooks
        .conflicts
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        sqlx::query("DO $$ BEGIN RAISE EXCEPTION 'write attempt' USING ERRCODE='40001'; END $$")
            .execute(connection)
            .await?;
    }
    Ok(())
}

fn presented_family(state: &LoginState, token: &str) -> Option<Uuid> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[&state.token_issuer]);
    validation.set_audience(&[&state.audience]);
    validation.leeway = 0;
    let claims = state
        .refresh_keys
        .iter()
        .find_map(|key| decode::<Value>(token, key, &validation).ok())?
        .claims;
    if claims.get("type")?.as_str()? != "refresh" {
        return None;
    }
    Uuid::parse_str(claims.get("sub")?.as_str()?).ok()?;
    if claims.get("jti")?.as_str()?.is_empty() {
        return None;
    }
    Uuid::parse_str(claims.get("family_id")?.as_str()?).ok()
}

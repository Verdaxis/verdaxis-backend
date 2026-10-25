//! S6 invitation acceptance: consume the administrator's one-time invitation
//! secret and issue the recipient's first session.
//!
//! The invitation secret lives in the shared `users.password_reset_token_hash`
//! and `users.password_reset_expires` columns that Python's admin-invitation
//! flow writes and resolves (coexistence C9). This component reads and clears
//! that column and never reads or writes `password_reset_tokens`, the
//! Rust-owned table of the ordinary reset flow. An invitation secret therefore
//! cannot be spent as a reset and a reset secret cannot be spent as an
//! invitation.
//!
//! One transaction holds the canonical session locks - device advisory lock,
//! user row, then the user's referral row and refresh families - and does every
//! write in that order: password hash, verification, forced-change flag, login
//! time, referral advance, referral code, login day, audit append, and one new
//! session family. Every older family of that user is revoked and the
//! revocation cutoff advances monotonically (plan 8.2-8.4), so a session that
//! existed before the claim cannot be refreshed afterwards.
//!
//! Refusals are one 400 `INVITATION_INVALID` for an unknown, expired, spent,
//! reissued or ineligible invitation, so the route is not an oracle for which
//! account states exist. The response body is `{access_token, token_type}` and
//! both session cookies, as Python returns.

use super::email_verification::{auth_json, client_address};
use crate::ApiError;
use crate::auth::{
    cookies::AuthCookies,
    error::{AuthCode, AuthFailure},
    issue::{REFRESH_TOKEN_DAYS, TokenIssuer},
    limits::SharedLimits,
    lock::WriteConflict,
    origin::require_trusted_origin,
    password, store,
    token_bytes::{generate_token, token_digest},
};
use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Duration, SubsecRound, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use std::{net::SocketAddr, sync::Arc};
use uuid::Uuid;

/// The invitation-secret bounds of Python's request schema, measured in bytes.
///
/// Python counts characters; counting bytes refuses a multi-byte secret Python
/// would accept and never accepts one Python refuses. An administrator
/// invitation secret is 43 ASCII characters (`secrets.token_urlsafe(32)`).
const TOKEN_MIN_BYTES: usize = 32;
const TOKEN_MAX_BYTES: usize = 128;
/// The agreement an accepted invitation records, copied from Python's audit.
const TERMS_PATH: &str = "/en/terms";
const PRIVACY_PATH: &str = "/en/privacy";
const AGREEMENT_VERSION: &str = "2026-08-05";
/// Python's registry name for this audit action.
const AUDIT_ACTION: &str = "user.invitation_accepted";

#[derive(Clone)]
pub struct InvitationAcceptState {
    pool: PgPool,
    environment: Arc<str>,
    issuer: Arc<TokenIssuer>,
    /// `FRONTEND_URL`; the audit records the absolute Terms and Privacy links.
    frontend_url: Arc<str>,
    pub limits: SharedLimits,
}

impl InvitationAcceptState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        environment: &str,
        secret: &str,
        issuer: &str,
        audience: &str,
        frontend_url: &str,
        limits: SharedLimits,
    ) -> Self {
        Self {
            pool,
            environment: environment.into(),
            issuer: Arc::new(TokenIssuer::new(secret, issuer, audience)),
            frontend_url: frontend_url.into(),
            limits,
        }
    }
}

pub fn router(state: InvitationAcceptState) -> Router {
    Router::new()
        .route("/api/auth/invitations/accept", post(accept))
        .route("/api/auth/invitations/accept/", post(accept))
        .with_state(state)
}

#[derive(Deserialize)]
struct Acceptance {
    token: String,
    new_password: String,
    accept_terms: bool,
}

fn validation() -> ApiError {
    AuthFailure::new(AuthCode::ValidationFailed).into()
}

fn invalid() -> ApiError {
    AuthFailure::new(AuthCode::InvitationInvalid).into()
}

fn database_unavailable() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        ..ApiError::database()
    }
}

/// The single locked row an invitation names, with the organization state the
/// eligibility rule reads. The organization columns are absent when the account
/// has no organization; that state is an ordinary ineligible invitation, not a
/// read failure.
#[derive(sqlx::FromRow)]
struct Invitation {
    id: Uuid,
    role: Option<String>,
    organization_id: Option<Uuid>,
    organization_type: Option<String>,
    organization_status: Option<String>,
    organization_provenance: Option<String>,
    email_verified: bool,
    must_change_password: bool,
    status: String,
    referral_code: Option<String>,
    password_changed_at: Option<DateTime<Utc>>,
}

/// The columns of [`Invitation`]. The organization is joined on the user's own
/// `organization_id`, so the pair Python compares is the pair read here.
const SELECT_INVITATION: &str = "SELECT u.id, u.role::text AS role, u.organization_id, \
     o.type::text AS organization_type, \
     o.verification_status AS organization_status, \
     o.provenance::text AS organization_provenance, \
     u.email_verified, u.must_change_password, u.status::text AS status, \
     u.referral_code, u.password_changed_at \
     FROM users u LEFT JOIN organizations o ON o.id = u.organization_id \
     WHERE u.password_reset_token_hash = $1 AND u.password_reset_expires > $2 \
     FOR UPDATE OF u";

/// Python's `organization_type_matches_role`: buy-side types belong to a BUYER
/// account, `FUEL_SUPPLIER` and `FUEL_TRADER` to a SUPPLIER account.
fn role_matches_organization(role: &str, organization_type: &str) -> bool {
    match role {
        "SUPPLIER" => matches!(organization_type, "FUEL_SUPPLIER" | "FUEL_TRADER"),
        "BUYER" => matches!(
            organization_type,
            "SHIPPING_LINE" | "SHIP_MANAGER" | "FUEL_BUYER" | "CHARTERER"
        ),
        _ => false,
    }
}

/// Python's `_is_unclaimed_admin_invitation`: an approved, unverified,
/// forced-change account whose role matches its approved live organization. A
/// spent invitation fails here too, because acceptance clears the flags.
fn eligible(invitation: &Invitation) -> bool {
    let (Some(role), Some(organization_type), Some(status), Some(provenance)) = (
        invitation.role.as_deref(),
        invitation.organization_type.as_deref(),
        invitation.organization_status.as_deref(),
        invitation.organization_provenance.as_deref(),
    ) else {
        return false;
    };
    invitation.status == "APPROVED"
        && !invitation.email_verified
        && invitation.must_change_password
        && invitation.organization_id.is_some()
        && status == "APPROVED"
        && matches!(provenance, "REAL" | "UNKNOWN")
        && role_matches_organization(role, organization_type)
}

/// Python's password policy for this route: at least 8 characters, at most
/// 1024 UTF-8 bytes, at least one uppercase letter and at least one digit.
///
/// The character classes are ASCII. Python's `\d` also matches non-ASCII
/// decimal digits, so this rule refuses some passwords Python would accept and
/// never accepts one Python would refuse.
pub fn strong_password(value: &str) -> bool {
    password::validate_bytes(value).is_ok()
        && value.bytes().any(|byte| byte.is_ascii_uppercase())
        && value.bytes().any(|byte| byte.is_ascii_digit())
}

async fn accept(
    State(state): State<InvitationAcceptState>,
    jar: CookieJar,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    request_id: Option<Extension<String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let presented = AuthCookies::read(&jar);
    require_trusted_origin(
        &headers,
        &state.environment,
        presented.cookie_authenticated(),
    )
    .map_err(|_| ApiError::from(AuthFailure::new(AuthCode::BrowserOriginRejected)))?;
    let acceptance: Acceptance = auth_json(&headers, body).await?;
    if !acceptance.accept_terms
        || !(TOKEN_MIN_BYTES..=TOKEN_MAX_BYTES).contains(&acceptance.token.len())
        || !strong_password(&acceptance.new_password)
    {
        return Err(validation());
    }
    // The endpoint quota is charged after the browser-authority check and the
    // typed validation, so a refused request spends nothing, and before any
    // database read.
    let client = client_address(peer, &headers);
    state.limits.check_endpoint(
        &client,
        "/api/auth/invitations/accept",
        Utc::now().timestamp().max(0) as u64,
    )?;
    let device = presented
        .valid_device()
        .map(str::to_owned)
        .unwrap_or_else(generate_token);
    let device_hash = token_digest(&device);
    let digest = token_digest(&acceptance.token);
    let request_id = correlation_id(request_id.as_deref().map(String::as_str), &headers);
    for attempt in 0..2 {
        match issue(
            &state,
            &acceptance,
            &digest,
            &device_hash,
            &client,
            request_id.as_deref(),
        )
        .await
        {
            Ok((access, refresh)) => {
                let jar =
                    AuthCookies::for_environment(&state.environment).set(jar, &refresh, &device);
                return Ok((
                    jar,
                    Json(json!({"access_token": access, "token_type": "bearer"})),
                )
                    .into_response());
            }
            Err(AcceptError::Refused(error)) => return Err(error),
            Err(AcceptError::Database(error)) if attempt == 0 && error.is_write_conflict() => {
                continue;
            }
            Err(AcceptError::Database(_)) => return Err(database_unavailable()),
        }
    }
    Err(database_unavailable())
}

enum AcceptError {
    Refused(ApiError),
    Database(sqlx::Error),
}

impl From<sqlx::Error> for AcceptError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<ApiError> for AcceptError {
    fn from(error: ApiError) -> Self {
        Self::Refused(error)
    }
}

/// The correlation id Python stores: the one the request middleware assigned,
/// otherwise an acceptable supplied header value, otherwise none.
fn correlation_id(assigned: Option<&str>, headers: &HeaderMap) -> Option<String> {
    let acceptable = |value: &str| {
        (1..=36).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    };
    assigned
        .filter(|value| acceptable(value))
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .filter(|value| acceptable(value))
                .map(str::to_owned)
        })
}

async fn issue(
    state: &InvitationAcceptState,
    acceptance: &Acceptance,
    digest: &str,
    device_hash: &str,
    client: &str,
    request_id: Option<&str>,
) -> Result<(String, String), AcceptError> {
    let mut tx = state.pool.begin().await?;
    // The serving pool defaults to read-only; writes opt in per transaction.
    sqlx::query("SET TRANSACTION READ WRITE")
        .execute(&mut *tx)
        .await?;
    store::lock_device(&mut tx, device_hash).await?;
    let now = Utc::now();
    let Some(invitation) = sqlx::query_as::<_, Invitation>(SELECT_INVITATION)
        .bind(digest)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Err(invalid().into());
    };
    if !eligible(&invitation) {
        return Err(invalid().into());
    }
    let organization_id = invitation
        .organization_id
        .expect("eligibility requires an organization");
    // Hash only after the invitation is known to be usable, so an unknown or
    // spent secret never occupies a password worker.
    let hash = password::hash(&acceptance.new_password)
        .await
        .map_err(|_| validation())?;
    // Referral rows before refresh rows: the order plan 8.2 names after the
    // user row.
    let referral: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status::text FROM referrals WHERE referred_user_id = $1 FOR UPDATE",
    )
    .bind(invitation.id)
    .fetch_optional(&mut *tx)
    .await?;
    let now = now.trunc_subsecs(6);
    let cutoff = store::revoke_user_authentication(
        &mut tx,
        invitation.id,
        now.max(invitation.password_changed_at.unwrap_or(now)),
    )
    .await?;
    sqlx::query(
        "UPDATE users SET password_hash = $2, password_reset_token_hash = NULL, \
         password_reset_expires = NULL, password_changed_at = $3, \
         must_change_password = false, email_verified = true, last_login = $4 \
         WHERE id = $1",
    )
    .bind(invitation.id)
    .bind(&hash)
    .bind(cutoff)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if let Some((id, status)) = referral
        && status == "SIGNED_UP"
    {
        sqlx::query("UPDATE referrals SET status = 'VERIFIED', verified_at = $2 WHERE id = $1")
            .bind(id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    if invitation.referral_code.as_deref().unwrap_or("").is_empty() {
        assign_referral_code(&mut tx, invitation.id).await?;
    }
    sqlx::query("INSERT INTO user_login_days (id,activity_date,user_id,organization_id,role,login_count,first_login_at,last_login_at) VALUES ($1,$2,$3,$4,$5,1,$6,$6) ON CONFLICT (activity_date,user_id) DO UPDATE SET login_count=user_login_days.login_count+1,first_login_at=LEAST(user_login_days.first_login_at,EXCLUDED.first_login_at),last_login_at=GREATEST(user_login_days.last_login_at,EXCLUDED.last_login_at)")
        .bind(Uuid::new_v4())
        .bind(now.date_naive())
        .bind(invitation.id)
        .bind(Some(organization_id))
        .bind(&invitation.role)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        invitation.id,
        organization_id,
        &state.frontend_url,
        client,
        request_id,
    )
    .await?;
    let family = Uuid::new_v4();
    let jti = generate_token();
    let access = state.issuer.access(invitation.id, now, Some(cutoff));
    let refresh = state
        .issuer
        .refresh(invitation.id, family, &jti, now, Some(cutoff));
    let issued = if now <= cutoff {
        cutoff + Duration::microseconds(1)
    } else {
        now
    };
    store::insert_refresh(
        &mut tx,
        &store::NewRefreshRow {
            user_id: invitation.id,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: device_hash.to_owned(),
            expires_at: issued + Duration::days(REFRESH_TOKEN_DAYS),
        },
    )
    .await?;
    tx.commit().await?;
    Ok((access, refresh))
}

/// Append the acceptance to the audit trail with Python's action name and
/// fields.
async fn audit(
    conn: &mut PgConnection,
    user_id: Uuid,
    organization_id: Uuid,
    frontend_url: &str,
    client: &str,
    request_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    let frontend = frontend_url.trim_end_matches('/');
    let changes = json!({
        "terms_accepted": true,
        "terms_url": format!("{frontend}{TERMS_PATH}"),
        "privacy_url": format!("{frontend}{PRIVACY_PATH}"),
        "agreement_version": AGREEMENT_VERSION,
        "organization_id": organization_id.to_string(),
    });
    sqlx::query(
        "INSERT INTO audit_logs \
         (id, user_id, action, resource_type, resource_id, changes, ip_address, request_id) \
         VALUES ($1, $2, $3, 'user', $4, $5, $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(AUDIT_ACTION)
    .bind(user_id.to_string())
    .bind(changes)
    .bind(client)
    .bind(request_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Give the claimed account a referral code when it has none.
///
/// Each probe runs in its own savepoint, so a unique-code collision stays local
/// and the next candidate can be tried inside the same transaction. Exhausting
/// the ten probes fails the acceptance instead of committing an account
/// without a code: nothing is consumed, and the recipient can retry.
async fn assign_referral_code(conn: &mut PgConnection, user_id: Uuid) -> Result<(), sqlx::Error> {
    for candidate in
        crate::crud::referrals::Candidates::new().take(crate::crud::referrals::COLLISION_PROBES)
    {
        sqlx::query("SAVEPOINT invitation_referral_code")
            .execute(&mut *conn)
            .await?;
        let result = sqlx::query("UPDATE users SET referral_code = $2 WHERE id = $1")
            .bind(user_id)
            .bind(candidate)
            .execute(&mut *conn)
            .await;
        match result {
            Ok(_) => {
                sqlx::query("RELEASE SAVEPOINT invitation_referral_code")
                    .execute(&mut *conn)
                    .await?;
                return Ok(());
            }
            Err(error)
                if error
                    .as_database_error()
                    .is_some_and(|error| error.is_unique_violation()) =>
            {
                sqlx::query("ROLLBACK TO SAVEPOINT invitation_referral_code")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("RELEASE SAVEPOINT invitation_referral_code")
                    .execute(&mut *conn)
                    .await?;
            }
            Err(error) => return Err(error),
        }
    }
    Err(sqlx::Error::RowNotFound)
}

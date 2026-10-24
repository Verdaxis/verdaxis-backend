//! A01/A02: the current principal's profile read and update.
//!
//! Runtime composition remains with the integration owner; this module is the
//! component adapter the owner mounts. The admission order is the PM's shared
//! one: authenticate, current account, then quota, then resource work.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, CurrentActor, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::{
        error::{authentication, refusal},
        transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
    },
    runtime::http::client_ip,
};

const ME_PATH: &str = "/api/auth/me";

/// The profile projection of the shared KYC decision (CF4_02). The real policy
/// module lands with Task 9; the declaration is the seam, never a second rule.
pub struct ProfileKycParty {
    pub user_id: Uuid,
    pub organization_id: Option<Uuid>,
    pub kyc_organization_id: Option<Uuid>,
    pub kyc_status: String,
}

/// The reason strings `ExecutionEligibilityResponse` allows. `NOT_APPLICABLE`
/// is a presentation state of the profile route, never a market decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KycReason {
    NotApplicable,
    NotEnforced,
    Approved,
    Rejected,
    Grace,
    Required,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct KycEligibilityView {
    pub eligible: bool,
    pub reason: KycReason,
    pub grace_expires_at: Option<DateTime<Utc>>,
}

/// One decision point for the profile route's eligibility projection.
pub trait KycProfilePolicy: Send + Sync + 'static {
    fn eligibility<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        party: ProfileKycParty,
        execution_gate_enabled: bool,
        now: DateTime<Utc>,
    ) -> CommandFuture<'a, KycEligibilityView, sqlx::Error>;
}

/// The shared public-projection probe and market-event producer the role
/// assignment needs (Task 6's declaration). Neither method commits.
pub trait ProfileMarketEvents: Send + Sync + 'static {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError>;
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError>;
}

pub struct ProfileAuthority {
    verification: VerificationSettings,
    limits: SharedLimits,
    policy: Arc<dyn KycProfilePolicy>,
    events: Arc<dyn ProfileMarketEvents>,
}

impl ProfileAuthority {
    pub fn new(
        verification: VerificationSettings,
        limits: SharedLimits,
        policy: Arc<dyn KycProfilePolicy>,
        events: Arc<dyn ProfileMarketEvents>,
    ) -> Self {
        Self {
            verification,
            limits,
            policy,
            events,
        }
    }
}

/// Exactly the fields the client contract reads, with Python's defaults.
#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub role: Option<String>,
    pub id: Uuid,
    pub status: String,
    pub organization_id: Option<Uuid>,
    pub referral_code: Option<String>,
    pub must_change_password: bool,
    pub kyc_status: String,
    pub execution_eligibility: KycEligibilityView,
}

/// One literal projection; a static statement keeps the SQL auditable and
/// lets `sqlx::query` accept it without a runtime injection audit.
const PROFILE_SELECT: &str = "SELECT id,email,first_name,last_name,role::text AS role,\
    status::text AS status,organization_id,referral_code,must_change_password,\
    kyc_status,kyc_organization_id FROM users WHERE id=$1";

/// The presentation rule Python applies before the policy is read: a role
/// outside the executing pair, or no organization, is not applicable.
fn not_applicable() -> KycEligibilityView {
    KycEligibilityView {
        eligible: false,
        reason: KycReason::NotApplicable,
        grace_expires_at: None,
    }
}

/// Load one profile and its eligibility decision on the caller's connection.
/// The read borrows the authenticated transaction; it writes nothing.
pub async fn select_profile(
    connection: &mut PgConnection,
    user: Uuid,
    execution_gate_enabled: bool,
    policy: &dyn KycProfilePolicy,
    now: DateTime<Utc>,
) -> Result<ProfileResponse, sqlx::Error> {
    let row = sqlx::query(PROFILE_SELECT)
        .bind(user)
        .fetch_one(&mut *connection)
        .await?;
    let role: Option<String> = row.get("role");
    let organization_id: Option<Uuid> = row.get("organization_id");
    let kyc_organization_id: Option<Uuid> = row.get("kyc_organization_id");
    let kyc_status: String = row.get("kyc_status");
    let execution_eligibility =
        if !matches!(role.as_deref(), Some("BUYER" | "SUPPLIER")) || organization_id.is_none() {
            not_applicable()
        } else {
            policy
                .eligibility(
                    connection,
                    ProfileKycParty {
                        user_id: user,
                        organization_id,
                        kyc_organization_id,
                        kyc_status,
                    },
                    execution_gate_enabled,
                    now,
                )
                .await?
        };
    Ok(ProfileResponse {
        email: row.get("email"),
        first_name: row.get("first_name"),
        last_name: row.get("last_name"),
        role,
        id: row.get("id"),
        status: row.get("status"),
        organization_id,
        referral_code: row.get("referral_code"),
        must_change_password: row.get("must_change_password"),
        kyc_status: row.get("kyc_status"),
        execution_eligibility,
    })
}

/// A validated `UserUpdate`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProfileUpdate {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub role: Option<String>,
}

/// `UserUpdate` is an all-optional body; only the three declared fields and
/// their declared value domain are accepted. Message wording is free.
pub fn validate_update(body: Value) -> Result<ProfileUpdate, ApiError> {
    let invalid = |code: &'static str, message: &'static str| {
        refusal(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    };
    let Some(object) = body.as_object() else {
        return Err(invalid(
            "VALIDATION_FAILED",
            "A profile object is required.",
        ));
    };
    let mut update = ProfileUpdate::default();
    for (field, value) in object {
        match field.as_str() {
            "first_name" | "last_name" => {
                let name = value
                    .as_str()
                    .ok_or_else(|| invalid("VALIDATION_FAILED", "Names must be strings."))?;
                if field == "first_name" {
                    update.first_name = Some(name.to_owned());
                } else {
                    update.last_name = Some(name.to_owned());
                }
            }
            "role" => {
                let role = value
                    .as_str()
                    .ok_or_else(|| invalid("VALIDATION_FAILED", "The role must be a string."))?;
                if !matches!(role, "BUYER" | "SUPPLIER" | "ADMIN") {
                    return Err(invalid(
                        "VALIDATION_FAILED",
                        "The role is not a known value.",
                    ));
                }
                update.role = Some(role.to_owned());
            }
            _ => {
                return Err(invalid(
                    "VALIDATION_FAILED",
                    "The field is not part of the profile update.",
                ));
            }
        }
    }
    Ok(update)
}

/// The ordered write: revalidated under the actor's locks, one commit.
struct UpdateProfile {
    actor: Uuid,
    update: ProfileUpdate,
    now: DateTime<Utc>,
    execution_gate_enabled: bool,
    policy: Arc<dyn KycProfilePolicy>,
    events: Arc<dyn ProfileMarketEvents>,
}

impl BusinessCommand for UpdateProfile {
    type Output = ProfileResponse;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            // Values only: no lock may be held across the preview boundary.
            let row =
                sqlx::query("SELECT organization_id,role::text AS role FROM users WHERE id=$1")
                    .bind(self.actor)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(|| {
                        AttemptError::Application(authentication(
                            crate::auth::AuthError::Credentials,
                        ))
                    })?;
            let organization: Option<Uuid> = row.get("organization_id");
            let role: Option<String> = row.get("role");
            // The once-only role rule, decided from values before any lock.
            if let Some(requested) = self.update.role.as_deref()
                && requested != role.as_deref().unwrap_or("\0")
                && (role.is_some() || requested == "ADMIN")
            {
                return Err(AttemptError::Application(refusal(
                    StatusCode::FORBIDDEN,
                    "PROFILE_ROLE_CHANGE_NOT_ALLOWED",
                    "Profile role changes are not allowed.",
                )));
            }
            Ok(OrganizationWritePlan::new(
                organization,
                [],
                [],
                serde_json::json!({"organization": organization, "role": role}),
            ))
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, ProfileResponse, AttemptError> {
        let policy = self.policy.clone();
        let events = self.events.clone();
        let update = clone_update(&self.update);
        let now = self.now;
        let gate = self.execution_gate_enabled;
        Box::pin(async move {
            let row = sqlx::query(PROFILE_SELECT)
                .bind(actor.id)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| {
                    AttemptError::Application(authentication(crate::auth::AuthError::Credentials))
                })?;
            let role: Option<String> = row.get("role");
            let organization_id: Option<Uuid> = row.get("organization_id");
            let requested = update.role.as_deref();
            // The role is assigned once, and never ADMIN. The preview decided
            // the same rule from values; under the locks it holds here.
            if let Some(requested) = requested
                && requested != role.as_deref().unwrap_or("\0")
                && (role.is_some() || requested == "ADMIN")
            {
                return Err(AttemptError::Application(refusal(
                    StatusCode::FORBIDDEN,
                    "PROFILE_ROLE_CHANGE_NOT_ALLOWED",
                    "Profile role changes are not allowed.",
                )));
            }
            let first = update.first_name.as_deref();
            let last = update.last_name.as_deref();
            let assigns_role = requested.is_some_and(|value| role.as_deref() != Some(value));
            // Python evaluates the public projection before the role changes
            // and again after the flush. Only a profile bound to an
            // organization has one; both probes sit inside this transaction.
            let public_before = match organization_id.filter(|_| assigns_role) {
                Some(_) => events.visible(conn, actor.id).await?,
                None => false,
            };
            sqlx::query("UPDATE users SET first_name=COALESCE($2,first_name),last_name=COALESCE($3,last_name),role=COALESCE($4,role) WHERE id=$1")
                .bind(actor.id)
                .bind(first)
                .bind(last)
                .bind(update.role.as_deref())
                .execute(&mut *conn)
                .await?;
            if assigns_role && let Some(organization) = organization_id {
                // The first role assignment changes public admission only when
                // the profile already has or gains a public projection.
                let public_after = events.visible(conn, actor.id).await?;
                if public_before || public_after {
                    events
                        .append(conn, actor.id, organization, public_before || public_after)
                        .await?;
                }
            }
            let profile = select_profile(conn, actor.id, gate, policy.as_ref(), now).await?;
            Ok(profile)
        })
    }
}

fn clone_update(update: &ProfileUpdate) -> ProfileUpdate {
    ProfileUpdate {
        first_name: update.first_name.clone(),
        last_name: update.last_name.clone(),
        role: update.role.clone(),
    }
}

pub fn routes(authority: ProfileAuthority) -> Router<AppState> {
    Router::new()
        .route(ME_PATH, get(me_route).put(me_put))
        .layer(Extension(Arc::new(authority)))
}

async fn me_route(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<ProfileAuthority>>,
    request: Request,
) -> Response {
    serve_read(state, authority, request).await
}

async fn me_put(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<ProfileAuthority>>,
    request: Request,
) -> Response {
    serve_update(state, authority, request).await
}

fn authenticated_error(error: AuthenticatedError) -> ApiError {
    match error {
        AuthenticatedError::Authentication(error) => authentication(error),
        AuthenticatedError::Database(error) => error,
        // A read holds no transaction of its own to lose.
        AuthenticatedError::AmbiguousCommit => ApiError::database(),
    }
}

async fn serve_read(
    state: AppState,
    authority: Arc<ProfileAuthority>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let _ = body;
    let now = Utc::now();
    let access = match verify_authorization(
        parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &authority.verification,
        now,
    ) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = client_ip(
        peer,
        parts
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let gate = state.config.kyc_execution_gate_enabled;
    let policy = authority.policy.clone();
    let user = access.subject;
    let limits = authority.limits.clone();
    let result = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("GET", ME_PATH, false),
        move |transaction| {
            Box::pin(async move {
                let actor = transaction.actor().clone();
                quota(
                    &limits,
                    "GET",
                    &client,
                    &actor,
                    now.timestamp().max(0) as u64,
                )?;
                select_profile(transaction.connection(), user, gate, policy.as_ref(), now)
                    .await
                    .map_err(|_| ApiError::database())
            })
        },
    )
    .await;
    match result {
        Ok(profile) => axum::Json(profile).into_response(),
        Err(error) => authenticated_error(error).into_response(),
    }
}

async fn serve_update(
    state: AppState,
    authority: Arc<ProfileAuthority>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let now = Utc::now();
    // The pinned body limit of the frozen inventory, refused before parsing.
    let raw = match axum::body::to_bytes(body, 131_072).await {
        Ok(raw) => raw,
        Err(_) => {
            return refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BODY_TOO_LARGE",
                "The profile body is too large.",
            )
            .into_response();
        }
    };
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        match crate::auth::request::parse_json_body(&raw) {
            Ok(payload) => payload,
            Err(error) => return error.into_response(),
        }
    };
    let authorization = parts
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // Authentication precedes typed validation, as FastAPI resolves the
    // security dependency before it validates the body model.
    let access = match verify_authorization(authorization.as_deref(), &authority.verification, now)
    {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let update = match validate_update(payload) {
        Ok(update) => update,
        Err(error) => return error.into_response(),
    };
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = client_ip(
        peer,
        parts
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    // Authority and typed input precede quota (PD4-T8-1); the pre-pass only
    // revalidates the current account and counts the call, and commits nothing.
    let limits = authority.limits.clone();
    let quota_client = client.clone();
    let denied = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("PUT", ME_PATH, false),
        move |transaction| {
            Box::pin(async move {
                quota(
                    &limits,
                    "PUT",
                    &quota_client,
                    transaction.actor(),
                    now.timestamp().max(0) as u64,
                )?;
                Ok::<(), ApiError>(())
            })
        },
    )
    .await;
    if let Err(error) = denied {
        return authenticated_error(error).into_response();
    }
    // The write owner revalidates the same token, account and preview under
    // the sorted locks; the token was already verified above.
    let access = match verify_authorization(authorization.as_deref(), &authority.verification, now)
    {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let command = UpdateProfile {
        actor: access.subject,
        update,
        now,
        execution_gate_enabled: state.config.kyc_execution_gate_enabled,
        policy: authority.policy.clone(),
        events: authority.events.clone(),
    };
    match crate::business::transaction::run_organization_write(&state, access, command).await {
        Ok(profile) => axum::Json(profile).into_response(),
        Err(error) => error.into_response(),
    }
}

fn quota(
    limits: &SharedLimits,
    method: &'static str,
    client: &str,
    actor: &CurrentActor,
    now: u64,
) -> Result<(), ApiError> {
    let user = actor.id.to_string();
    let organization = actor.organization_id.map(|id| id.to_string());
    limits.check_business(
        method,
        ME_PATH,
        BusinessLimitKeys {
            client,
            user: Some(&user),
            organization: organization.as_deref(),
            token: None,
        },
        now,
    )
}

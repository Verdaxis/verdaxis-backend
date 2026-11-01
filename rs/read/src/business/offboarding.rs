//! Administrator offboarding of one individual account, and its preview.
//!
//! `POST /api/auth/admin/users/{user_id}/offboard` ends one person's access in
//! one transaction of the ordered business owner
//! ([`super::transaction::run_organization_write`]):
//!
//! 1. discovery reads the target's organization and pending join requests;
//! 2. the owner locks organizations, then the actor and target users, then
//!    those join requests, each in ascending id ([`crate::auth::lock`]), and
//!    revalidates the actor; a changed discovery runs the whole attempt again;
//! 3. under those locks the command checks the target, sets `REJECTED` and the
//!    `offboarded_at` marker from the database clock, detaches the member and
//!    invalidates the current organization-bound KYC review (an unrecognized
//!    review state is kept and recorded instead), cuts off every credential
//!    ([`crate::auth::offboarding`]), closes pending join requests and the
//!    market-support grants bound to the person, runs the injected market hook,
//!    and appends the offboarding record, the audit record and the admission
//!    outbox event.
//!
//! Every step borrows the caller's transaction, so a failure anywhere rolls
//! back all of it. The response is sent only after the commit. An unknown
//! commit outcome is answered with `COMMIT_OUTCOME_UNKNOWN`; the client then
//! reads the user again instead of repeating the command.
//!
//! The market work is supplied through [`OffboardingMarketHook`]. No
//! implementation of it exists in this crate's serving code, and
//! [`crate::runtime::phase4::routes`] does not mount these routes; the
//! composition owner mounts [`router`] only with the real market
//! implementation.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Request, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    admission::{AdmissionEvents, AuditContext},
    audit::{AuditFact, append_audit},
    error::{authentication, refusal},
    membership::{invalidate_review, require_admin},
    status::{AccountStatus, StatusFact, append_status_transition},
    transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
};
// The case-reference contract is shared with the trade responsibility reassignment.
pub use crate::case_reference::{CASE_REFERENCE_MAX_LENGTH, CASE_REFERENCE_PREFIXES, CaseReference};
use crate::{
    ApiError, AppState,
    auth::{
        AuthError, AuthenticatedError, AuthenticatedRequest, CurrentActor, VerificationSettings,
        VerifiedAccess,
        offboarding::{UnusablePasswordHash, revoke_account_credentials},
        run_authenticated, verify_authorization,
    },
};

/// The command route.
pub const OFFBOARD_ROUTE: &str = "/api/auth/admin/users/{user_id}/offboard";
/// The preview route.
pub const PREVIEW_ROUTE: &str = "/api/auth/admin/users/{user_id}/offboarding-preview";

/// The reason codes a client may send. The UI offers exactly these.
pub const REASON_CODES: [&str; 4] = [
    "EMPLOYMENT_ENDED",
    "COMPANY_REQUEST",
    "SECURITY_CONCERN",
    "ACCOUNT_HOLDER_REQUEST",
];


/// The largest request body. The four fields fit well inside it, so a larger
/// body can only carry text the contract does not accept.
pub const REQUEST_BODY_LIMIT_BYTES: usize = 4096;

/// The bound of one market invalidation transaction. The preview refuses when
/// the rows the command withdraws or cancels (open order remainders, pending
/// trades) exceed it, because the command would refuse too. Confirmed and
/// delivered trades only inform the administrator: above the bound their
/// counts are reported as the bound, with `informational_counts_capped` set.
pub const PREVIEW_COUNT_LIMIT: i64 = 100;


/// The audit action name of a completed offboarding.
pub const AUDIT_ACTION: &str = "admin.user_offboarded";

/// The fixed reason written on records the command closes.
const CLOSURE_REASON: &str = "account_offboarded";

/// The fixed review note on join requests the command closes.
const JOIN_CLOSURE_NOTE: &str = "Account offboarded.";

/// The request header that carries an assisted market-support context.
const SUPPORT_CONTEXT_HEADER: &str = "x-verdaxis-market-support-context";

// ---------------------------------------------------------------------------
// Request contract.
// ---------------------------------------------------------------------------

/// One allowlisted reason code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReasonCode(&'static str);

impl ReasonCode {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        REASON_CODES
            .into_iter()
            .find(|code| *code == value)
            .map(Self)
            .ok_or_else(|| invalid("The reason code is not one of the accepted values."))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}


/// The validated body of one offboarding command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffboardRequest {
    pub reason_code: ReasonCode,
    pub evidence_case_ref: CaseReference,
    pub urgent: bool,
    /// The organization the administrator saw. `None` is an explicit JSON
    /// `null` for a target with no organization; the key is always required.
    pub expected_organization_id: Option<Uuid>,
}

impl OffboardRequest {
    const FIELDS: [&'static str; 4] = [
        "reason_code",
        "evidence_case_ref",
        "urgent",
        "expected_organization_id",
    ];

    /// Parse the JSON body. The object must have exactly the four fields; an
    /// unknown field is refused, so no free text can enter the record.
    pub fn parse(body: &[u8]) -> Result<Self, ApiError> {
        if body.len() > REQUEST_BODY_LIMIT_BYTES {
            return Err(body_too_large());
        }
        let value: Value = serde_json::from_slice(body)
            .map_err(|_| invalid("An offboarding object is required."))?;
        let Value::Object(fields) = value else {
            return Err(invalid("An offboarding object is required."));
        };
        if fields.len() != Self::FIELDS.len()
            || !Self::FIELDS.iter().all(|name| fields.contains_key(*name))
        {
            return Err(invalid(
                "The body must contain exactly reason_code, evidence_case_ref, urgent and expected_organization_id.",
            ));
        }
        Ok(Self {
            reason_code: ReasonCode::parse(text_field(&fields, "reason_code")?)?,
            evidence_case_ref: CaseReference::parse(text_field(&fields, "evidence_case_ref")?)
                .map_err(|_| {
                    invalid("The case reference must be a ticket identifier such as CASE-2026/0042.")
                })?,
            urgent: fields["urgent"]
                .as_bool()
                .ok_or_else(|| invalid("urgent must be true or false."))?,
            expected_organization_id: match &fields["expected_organization_id"] {
                Value::Null => None,
                Value::String(text) => Some(
                    Uuid::parse_str(text)
                        .map_err(|_| invalid("expected_organization_id must be a UUID or null."))?,
                ),
                _ => return Err(invalid("expected_organization_id must be a UUID or null.")),
            },
        })
    }
}

fn text_field<'a>(fields: &'a Map<String, Value>, name: &str) -> Result<&'a str, ApiError> {
    fields[name]
        .as_str()
        .ok_or_else(|| invalid("The reason code and the case reference must be strings."))
}

fn invalid(message: &'static str) -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_FAILED",
        message,
    )
}

fn body_too_large() -> ApiError {
    refusal(
        StatusCode::PAYLOAD_TOO_LARGE,
        "BODY_TOO_LARGE",
        "The offboarding body is too large.",
    )
}

fn not_found() -> ApiError {
    refusal(
        StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
        "The user was not found.",
    )
}

fn not_manageable() -> ApiError {
    refusal(
        StatusCode::FORBIDDEN,
        "ACCOUNT_NOT_MANAGEABLE",
        "Administrator accounts cannot be offboarded.",
    )
}

fn already_completed() -> ApiError {
    refusal(
        StatusCode::CONFLICT,
        "OFFBOARD_ALREADY_COMPLETED",
        "The account was already offboarded.",
    )
}

fn membership_changed() -> ApiError {
    refusal(
        StatusCode::CONFLICT,
        "OFFBOARD_MEMBERSHIP_CHANGED",
        "The account's organization changed; review it again.",
    )
}

fn support_context_refused() -> ApiError {
    refusal(
        StatusCode::FORBIDDEN,
        "MARKET_SUPPORT_MUTATION_NOT_ALLOWED",
        "This operation is not available in Market Support mode",
    )
}

// ---------------------------------------------------------------------------
// Market hook.
// ---------------------------------------------------------------------------

/// The person whose market exposure the hook ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserMarketScope {
    pub subject_id: Uuid,
    pub actor_id: Uuid,
    pub now: DateTime<Utc>,
}

/// What the market hook changed, recorded as counts in the audit record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketInvalidationSummary {
    pub withdrawn_orders: Vec<Uuid>,
    pub cancelled_trades: Vec<Uuid>,
    pub outbox_ids: Vec<Uuid>,
}

/// User-scoped market invalidation on the caller's transaction.
///
/// It runs after the account changes, while the organization and user row
/// locks are held, and takes its own market locks only after them. It must not
/// commit, must refuse work above its bound instead of truncating it, and must
/// report a changed market preview as a retryable database conflict so the
/// owner runs the whole attempt again.
pub trait OffboardingMarketHook: Send + Sync + 'static {
    fn invalidate_user<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        scope: &'a UserMarketScope,
    ) -> CommandFuture<'a, MarketInvalidationSummary, AttemptError>;
}

impl<H: OffboardingMarketHook> OffboardingMarketHook for Arc<H> {
    fn invalidate_user<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        scope: &'a UserMarketScope,
    ) -> CommandFuture<'a, MarketInvalidationSummary, AttemptError> {
        self.as_ref().invalidate_user(conn, scope)
    }
}

// ---------------------------------------------------------------------------
// The command.
// ---------------------------------------------------------------------------

/// One offboarding command. Every recorded time (the marker, the status
/// transition, the closures, the history row and the audit records) is the
/// database clock read by the marker update, after the row locks are held.
pub struct OffboardAccount<H, E> {
    pub subject_id: Uuid,
    pub request: OffboardRequest,
    /// Prepared before the transaction takes its first lock; every retried
    /// attempt reuses it.
    pub unusable_hash: UnusablePasswordHash,
    pub audit: AuditContext,
    pub market: H,
    pub events: E,
}

/// The committed result. It carries no secret, case reference or KYC detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OffboardResult {
    pub operation_id: Uuid,
    pub user_id: Uuid,
    pub former_organization_id: Option<Uuid>,
    pub status: &'static str,
    pub offboarded_at: DateTime<Utc>,
}

impl<H: OffboardingMarketHook, E: AdmissionEvents> BusinessCommand for OffboardAccount<H, E> {
    type Output = OffboardResult;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT organization_id,role::text AS role,status::text AS status,\
                 offboarded_at IS NOT NULL AS offboarded FROM users WHERE id=$1",
            )
            .bind(self.subject_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(not_found)?;
            let organization: Option<Uuid> = row.get("organization_id");
            let joins: Vec<Uuid> = sqlx::query_scalar(
                "SELECT id FROM organization_join_requests \
                 WHERE user_id=$1 AND status='PENDING' ORDER BY id",
            )
            .bind(self.subject_id)
            .fetch_all(&mut *conn)
            .await?;
            Ok(OrganizationWritePlan::new(
                organization,
                [self.subject_id],
                joins.iter().copied(),
                json!({
                    "organization": organization,
                    "role": row.get::<Option<String>, _>("role"),
                    "status": row.get::<String, _>("status"),
                    "offboarded": row.get::<bool, _>("offboarded"),
                    "joins": joins,
                }),
            ))
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move { self.offboard(conn, actor).await })
    }
}

/// The target as read under its row lock.
struct Target {
    previous: AccountStatus,
    role: Option<String>,
    organization: Option<Uuid>,
}

/// One completed offboarding: its identifier and its database time.
struct Operation {
    id: Uuid,
    at: DateTime<Utc>,
}

/// The account-side changes, recorded as counts in the audit record.
struct AccountChanges {
    kyc_review_invalidated: bool,
    kyc_state_unrecognized: bool,
    refresh_families_revoked: u64,
    reset_tokens_removed: u64,
    join_requests_closed: usize,
    support_contexts_closed: usize,
    support_authorizations_revoked: usize,
    staff_capabilities_revoked: usize,
}

impl<H: OffboardingMarketHook, E: AdmissionEvents> OffboardAccount<H, E> {
    async fn offboard(
        &self,
        conn: &mut PgConnection,
        actor: &CurrentActor,
    ) -> Result<OffboardResult, AttemptError> {
        require_admin(actor)?;
        require_unmarked_actor(conn, actor.id).await?;
        if actor.id == self.subject_id {
            return Err(not_manageable().into());
        }
        let target = self.revalidate_target(conn).await?;
        let visible_before = self.events.visible(conn, self.subject_id).await?;
        let at = self.mark(conn).await?;
        let changes = self.change_account(conn, actor, &target, at).await?;
        let market = self
            .market
            .invalidate_user(
                conn,
                &UserMarketScope {
                    subject_id: self.subject_id,
                    actor_id: actor.id,
                    now: at,
                },
            )
            .await?;
        let visible_after = self.events.visible(conn, self.subject_id).await?;
        if let Some(organization) = target.organization {
            self.events
                .append(
                    conn,
                    self.subject_id,
                    organization,
                    visible_before || visible_after,
                )
                .await?;
        }
        let operation = Operation {
            id: Uuid::new_v4(),
            at,
        };
        self.record(conn, actor, &target, &operation).await?;
        self.append_audit_record(conn, actor, &target, &operation, &changes, &market)
            .await?;
        Ok(OffboardResult {
            operation_id: operation.id,
            user_id: self.subject_id,
            former_organization_id: target.organization,
            status: AccountStatus::Rejected.as_str(),
            offboarded_at: at,
        })
    }

    /// Check the locked target against the request. The order of the checks
    /// is part of the contract: unknown, administrator, already offboarded,
    /// then a changed organization.
    async fn revalidate_target(&self, conn: &mut PgConnection) -> Result<Target, AttemptError> {
        let row = sqlx::query(
            "SELECT status::text AS status,role::text AS role,organization_id,\
             offboarded_at IS NOT NULL AS offboarded FROM users WHERE id=$1",
        )
        .bind(self.subject_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(not_found)?;
        let role: Option<String> = row.get("role");
        if role.as_deref() == Some("ADMIN") {
            return Err(not_manageable().into());
        }
        if row.get::<bool, _>("offboarded") {
            return Err(already_completed().into());
        }
        let previous = match row.get::<String, _>("status").as_str() {
            "PENDING" => AccountStatus::Pending,
            "APPROVED" => AccountStatus::Approved,
            "REJECTED" => AccountStatus::Rejected,
            _ => {
                return Err(refusal(
                    StatusCode::CONFLICT,
                    "ACCOUNT_STATE_INVALID",
                    "The account state cannot be changed.",
                )
                .into());
            }
        };
        let organization: Option<Uuid> = row.get("organization_id");
        if organization != self.request.expected_organization_id {
            return Err(membership_changed().into());
        }
        Ok(Target {
            previous,
            role,
            organization,
        })
    }

    /// Set the rejected state, detach the member and set the marker in one
    /// statement, so both marker constraints hold for the written row. The
    /// time is the database clock now that the locks are held; it becomes the
    /// time of every record this command writes.
    async fn mark(&self, conn: &mut PgConnection) -> Result<DateTime<Utc>, AttemptError> {
        let at = sqlx::query_scalar(
            "UPDATE users SET status='REJECTED',organization_id=NULL,\
             offboarded_at=clock_timestamp() WHERE id=$1 RETURNING offboarded_at",
        )
        .bind(self.subject_id)
        .fetch_one(&mut *conn)
        .await?;
        Ok(at)
    }

    async fn change_account(
        &self,
        conn: &mut PgConnection,
        actor: &CurrentActor,
        target: &Target,
        at: DateTime<Utc>,
    ) -> Result<AccountChanges, AttemptError> {
        // The shared review transition refuses a stored state outside its
        // closed domain. Offboarding must not wait for an operator repair, so
        // such a state is kept as it is and recorded in the audit instead.
        let kyc_status: Option<String> =
            sqlx::query_scalar("SELECT kyc_status::text FROM users WHERE id=$1")
                .bind(self.subject_id)
                .fetch_one(&mut *conn)
                .await?;
        let kyc_state_unrecognized = !matches!(
            kyc_status.as_deref(),
            Some("PENDING" | "SUBMITTED" | "APPROVED" | "REJECTED")
        );
        // A detached target has no organization-bound review to invalidate.
        let kyc_review_invalidated = if target.organization.is_some() && !kyc_state_unrecognized {
            invalidate_review(conn, self.subject_id, actor.id, CLOSURE_REASON, at).await?
        } else {
            false
        };
        append_status_transition(
            conn,
            &StatusFact {
                id: Uuid::new_v4(),
                user_id: self.subject_id,
                organization_id: target.organization,
                role: target.role.clone(),
                from: Some(target.previous),
                to: AccountStatus::Rejected,
                at,
            },
        )
        .await?;
        let credentials = revoke_account_credentials(conn, self.subject_id, &self.unusable_hash)
            .await
            .map_err(|error| match error {
                sqlx::Error::RowNotFound => AttemptError::Application(not_found()),
                other => AttemptError::Database(other),
            })?;
        let join_requests_closed = sqlx::query_scalar::<_, Uuid>(
            "UPDATE organization_join_requests SET status='REJECTED',reviewed_by=$2,\
             reviewed_at=$3,review_note=$4 WHERE user_id=$1 AND status='PENDING' RETURNING id",
        )
        .bind(self.subject_id)
        .bind(actor.id)
        .bind(at)
        .bind(JOIN_CLOSURE_NOTE)
        .fetch_all(&mut *conn)
        .await?
        .len();
        let support_contexts_closed = sqlx::query_scalar::<_, Uuid>(
            "UPDATE market_support_contexts SET status='REVOKED',ended_at=$2,version=version+1 \
             WHERE status='ACTIVE' AND (actor_user_id=$1 OR accountable_user_id=$1) RETURNING id",
        )
        .bind(self.subject_id)
        .bind(at)
        .fetch_all(&mut *conn)
        .await?
        .len();
        let support_authorizations_revoked = sqlx::query_scalar::<_, Uuid>(
            "UPDATE market_support_authorizations SET status='REVOKED',revoked_at=$2,\
             revoked_by_actor_user_id=$3,revocation_reason=$4 \
             WHERE status='ACTIVE' AND accountable_user_id=$1 RETURNING id",
        )
        .bind(self.subject_id)
        .bind(at)
        .bind(actor.id)
        .bind(CLOSURE_REASON)
        .fetch_all(&mut *conn)
        .await?
        .len();
        let staff_capabilities_revoked = sqlx::query_scalar::<_, Uuid>(
            "UPDATE staff_capability_assignments SET revoked_at=$2,revoked_by_user_id=$3,\
             revocation_reason=$4 WHERE user_id=$1 AND revoked_at IS NULL RETURNING id",
        )
        .bind(self.subject_id)
        .bind(at)
        .bind(actor.id)
        .bind(CLOSURE_REASON)
        .fetch_all(&mut *conn)
        .await?
        .len();
        Ok(AccountChanges {
            kyc_review_invalidated,
            kyc_state_unrecognized,
            refresh_families_revoked: credentials.refresh_families_revoked,
            reset_tokens_removed: credentials.reset_tokens_removed,
            join_requests_closed,
            support_contexts_closed,
            support_authorizations_revoked,
            staff_capabilities_revoked,
        })
    }

    async fn record(
        &self,
        conn: &mut PgConnection,
        actor: &CurrentActor,
        target: &Target,
        operation: &Operation,
    ) -> Result<(), AttemptError> {
        sqlx::query(
            "INSERT INTO individual_offboardings(operation_id,subject_user_id,\
             former_organization_id,actor_user_id,occurred_at,reason_code,evidence_case_ref,urgent) \
             VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(operation.id)
        .bind(self.subject_id)
        .bind(target.organization)
        .bind(actor.id)
        .bind(operation.at)
        .bind(self.request.reason_code.as_str())
        .bind(self.request.evidence_case_ref.as_str())
        .bind(self.request.urgent)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    async fn append_audit_record(
        &self,
        conn: &mut PgConnection,
        actor: &CurrentActor,
        target: &Target,
        operation: &Operation,
        changes: &AccountChanges,
        market: &MarketInvalidationSummary,
    ) -> Result<(), AttemptError> {
        append_audit(
            conn,
            &AuditFact {
                id: Uuid::new_v4(),
                actor_id: Some(actor.id),
                action: AUDIT_ACTION.into(),
                resource_type: "user".into(),
                resource_id: Some(self.subject_id.to_string()),
                changes: Some(json!({
                    "operation_id": operation.id,
                    "status": {"from": target.previous.as_str(), "to": "REJECTED"},
                    "organization_id": {"from": target.organization, "to": null},
                    "reason_code": self.request.reason_code.as_str(),
                    "evidence_case_ref": self.request.evidence_case_ref.as_str(),
                    "urgent": self.request.urgent,
                    "kyc_review_invalidated": changes.kyc_review_invalidated,
                    "kyc_state_unrecognized": changes.kyc_state_unrecognized,
                    "refresh_families_revoked": changes.refresh_families_revoked,
                    "reset_tokens_removed": changes.reset_tokens_removed,
                    "join_requests_closed": changes.join_requests_closed,
                    "support_contexts_closed": changes.support_contexts_closed,
                    "support_authorizations_revoked": changes.support_authorizations_revoked,
                    "staff_capabilities_revoked": changes.staff_capabilities_revoked,
                    "withdrawn_orders": market.withdrawn_orders.len(),
                    "cancelled_trades": market.cancelled_trades.len(),
                    "market_events": market.outbox_ids.len(),
                })),
                ip_address: self.audit.ip_address.clone(),
                request_id: self.audit.request_id.clone(),
                at: operation.at,
            },
        )
        .await?;
        Ok(())
    }
}

/// The owner's account check knows the status and cutoffs. An administrator
/// with the marker is refused here as well, under the same row lock.
async fn require_unmarked_actor(conn: &mut PgConnection, actor: Uuid) -> Result<(), AttemptError> {
    let marked: Option<bool> =
        sqlx::query_scalar("SELECT offboarded_at IS NOT NULL FROM users WHERE id=$1")
            .bind(actor)
            .fetch_optional(&mut *conn)
            .await?;
    match marked {
        Some(false) => Ok(()),
        Some(true) => Err(authentication(AuthError::Rejected).into()),
        None => Err(authentication(AuthError::Credentials).into()),
    }
}

// ---------------------------------------------------------------------------
// The preview.
// ---------------------------------------------------------------------------

/// Advisory counts for the administrator's confirmation. No target or market
/// row is locked; the command discovers and locks its own scope. The read runs
/// in the authenticated request transaction, which holds the administrator's
/// own user row for its short duration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OffboardingPreview {
    pub user_id: Uuid,
    pub email: String,
    pub status: String,
    pub organization_id: Option<Uuid>,
    pub organization_name: Option<String>,
    pub offboarded_at: Option<DateTime<Utc>>,
    pub open_owned_orders: i64,
    pub pending_trades: i64,
    /// Pending trades of the person under an active freeze. Each one blocks
    /// the command: it refuses with `409 MARKET_INVALIDATION_TRADE_FROZEN`
    /// until an administrator clears the freeze.
    pub frozen_pending_trades: i64,
    pub confirmed_trades: i64,
    pub delivered_trades: i64,
    /// True when a confirmed or delivered count exceeded the bound and is
    /// reported as [`PREVIEW_COUNT_LIMIT`].
    pub informational_counts_capped: bool,
}

/// Read the preview of one target. The caller has checked the administrator.
pub async fn preview(
    conn: &mut PgConnection,
    actor: &CurrentActor,
    subject: Uuid,
) -> Result<OffboardingPreview, ApiError> {
    require_admin(actor)?;
    if actor.id == subject {
        return Err(not_manageable());
    }
    let database = |error: sqlx::Error| {
        super::error::database_failure(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
        )
    };
    let row = sqlx::query(
        "SELECT u.email,u.role::text AS role,u.status::text AS status,u.organization_id,\
         o.name AS organization_name,u.offboarded_at \
         FROM users u LEFT JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1",
    )
    .bind(subject)
    .fetch_optional(&mut *conn)
    .await
    .map_err(database)?
    .ok_or_else(not_found)?;
    if row.get::<Option<String>, _>("role").as_deref() == Some("ADMIN") {
        return Err(not_manageable());
    }
    // Each count stops one row past the bound.
    let counts = sqlx::query(
        "SELECT \
         (SELECT count(*) FROM (SELECT 1 FROM orderbook_orders WHERE owner_user_id=$1 \
          AND status IN ('OPEN','PARTIALLY_FILLED') LIMIT $2) AS rows) AS open_owned_orders,\
         (SELECT count(*) FROM (SELECT 1 FROM trades WHERE (buyer_user_id=$1 OR seller_user_id=$1) \
          AND status='PENDING_CONFIRMATION' LIMIT $2) AS rows) AS pending_trades,\
         (SELECT count(*) FROM (SELECT 1 FROM trades t WHERE (t.buyer_user_id=$1 OR t.seller_user_id=$1) \
          AND t.status='PENDING_CONFIRMATION' AND EXISTS(SELECT 1 FROM trade_freezes f \
          WHERE f.trade_id=t.id AND f.status='ACTIVE') LIMIT $2) AS rows) AS frozen_pending_trades,\
         (SELECT count(*) FROM (SELECT 1 FROM trades WHERE (buyer_user_id=$1 OR seller_user_id=$1) \
          AND status='CONFIRMED' LIMIT $2) AS rows) AS confirmed_trades,\
         (SELECT count(*) FROM (SELECT 1 FROM trades WHERE (buyer_user_id=$1 OR seller_user_id=$1) \
          AND status='DELIVERED' LIMIT $2) AS rows) AS delivered_trades",
    )
    .bind(subject)
    .bind(PREVIEW_COUNT_LIMIT + 1)
    .fetch_one(&mut *conn)
    .await
    .map_err(database)?;
    let open_owned_orders: i64 = counts.get("open_owned_orders");
    let pending_trades: i64 = counts.get("pending_trades");
    if open_owned_orders > PREVIEW_COUNT_LIMIT || pending_trades > PREVIEW_COUNT_LIMIT {
        return Err(refusal(
            StatusCode::CONFLICT,
            "OFFBOARD_PREVIEW_LIMIT",
            "The account has more open orders or pending trades than one offboarding can process.",
        ));
    }
    let confirmed_trades: i64 = counts.get("confirmed_trades");
    let delivered_trades: i64 = counts.get("delivered_trades");
    Ok(OffboardingPreview {
        user_id: subject,
        email: row.get("email"),
        status: row.get("status"),
        organization_id: row.get("organization_id"),
        organization_name: row.get("organization_name"),
        offboarded_at: row.get("offboarded_at"),
        open_owned_orders,
        pending_trades,
        frozen_pending_trades: counts.get("frozen_pending_trades"),
        confirmed_trades: confirmed_trades.min(PREVIEW_COUNT_LIMIT),
        delivered_trades: delivered_trades.min(PREVIEW_COUNT_LIMIT),
        informational_counts_capped: confirmed_trades > PREVIEW_COUNT_LIMIT
            || delivered_trades > PREVIEW_COUNT_LIMIT,
    })
}

// ---------------------------------------------------------------------------
// Routes.
// ---------------------------------------------------------------------------

/// The market implementation and the admission event producer.
pub struct OffboardingService<H, E> {
    pub market: Arc<H>,
    pub events: Arc<E>,
}

impl<H, E> Clone for OffboardingService<H, E> {
    fn clone(&self) -> Self {
        Self {
            market: self.market.clone(),
            events: self.events.clone(),
        }
    }
}

struct RouteState<H, E> {
    app: AppState,
    verification: Arc<VerificationSettings>,
    service: OffboardingService<H, E>,
}

impl<H, E> Clone for RouteState<H, E> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            verification: self.verification.clone(),
            service: self.service.clone(),
        }
    }
}

/// Component router only. Nothing mounts it until the composition owner
/// supplies the real market implementation.
pub fn router<H: OffboardingMarketHook, E: AdmissionEvents>(
    app: AppState,
    verification: VerificationSettings,
    service: OffboardingService<H, E>,
) -> Router {
    Router::new()
        .route(OFFBOARD_ROUTE, post(offboard_route::<H, E>))
        .route(PREVIEW_ROUTE, get(preview_route::<H, E>))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            service,
        })
}

async fn offboard_route<H: OffboardingMarketHook, E: AdmissionEvents>(
    State(state): State<RouteState<H, E>>,
    Path(user_id): Path<String>,
    request: Request,
) -> Response {
    match offboard(state, user_id, request).await {
        Ok(result) => axum::Json(result).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn preview_route<H: OffboardingMarketHook, E: AdmissionEvents>(
    State(state): State<RouteState<H, E>>,
    Path(user_id): Path<String>,
    request: Request,
) -> Response {
    match read_preview(state, user_id, request).await {
        Ok(result) => axum::Json(result).into_response(),
        Err(error) => error.into_response(),
    }
}

fn bearer(
    request: &Parts,
    state_verification: &VerificationSettings,
    now: DateTime<Utc>,
) -> Result<VerifiedAccess, ApiError> {
    verify_authorization(
        request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        state_verification,
        now,
    )
    .map_err(authentication)
}

/// Authenticate, refuse an assisted context, and check the current account and
/// the administrator role under the actor's row lock. Only then is the target
/// identifier read, so an unauthorized caller learns nothing about it.
async fn admit<H, E>(
    state: &RouteState<H, E>,
    request: &Parts,
    method: &'static str,
    template: &'static str,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let access = bearer(request, &state.verification, now)?;
    refuse_assisted_context(request)?;
    run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new(method, template, false),
        |tx| Box::pin(async move { require_admin(tx.actor()) }),
    )
    .await
    .map_err(authenticated_failure)
}

/// No assisted market-support context authorizes these routes.
fn refuse_assisted_context(request: &Parts) -> Result<(), ApiError> {
    let assisted = request
        .headers
        .get(SUPPORT_CONTEXT_HEADER)
        .is_some_and(|value| !value.is_empty());
    if assisted {
        return Err(support_context_refused());
    }
    Ok(())
}

fn authenticated_failure(error: AuthenticatedError) -> ApiError {
    match error {
        AuthenticatedError::Authentication(error) => authentication(error),
        AuthenticatedError::Database(error) => error,
        AuthenticatedError::AmbiguousCommit => ApiError::database(),
    }
}

fn target(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| invalid("The user ID must be a UUID."))
}

fn audit_context(request: &Parts) -> AuditContext {
    let peer = request
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = crate::runtime::http::client_ip(
        peer,
        request
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    AuditContext {
        ip_address: Some(client),
        request_id: request.extensions.get::<String>().cloned().or_else(|| {
            request
                .headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        }),
    }
}

async fn offboard<H: OffboardingMarketHook, E: AdmissionEvents>(
    state: RouteState<H, E>,
    user_id: String,
    request: Request,
) -> Result<OffboardResult, ApiError> {
    let now = Utc::now();
    let (parts, body) = request.into_parts();
    admit(&state, &parts, "POST", OFFBOARD_ROUTE, now).await?;
    let subject = target(&user_id)?;
    let audit = audit_context(&parts);
    // The ordered owner revalidates this access under the canonical locks.
    let access = bearer(&parts, &state.verification, now)?;
    if access.subject == subject {
        return Err(not_manageable());
    }
    let body = axum::body::to_bytes(body, REQUEST_BODY_LIMIT_BYTES)
        .await
        .map_err(|_| body_too_large())?;
    let request = OffboardRequest::parse(&body)?;
    // Before the first lock of the offboarding transaction, and only after the
    // caller is admitted and the request is valid: the password workers can be
    // busy, and that wait must not hold any row.
    let unusable_hash = UnusablePasswordHash::prepare()
        .await
        .map_err(|_| ApiError::database())?;
    super::transaction::run_organization_write(
        &state.app,
        access,
        OffboardAccount {
            subject_id: subject,
            request,
            unusable_hash,
            audit,
            market: state.service.market.clone(),
            events: state.service.events.clone(),
        },
    )
    .await
}

async fn read_preview<H, E>(
    state: RouteState<H, E>,
    user_id: String,
    request: Request,
) -> Result<OffboardingPreview, ApiError> {
    let now = Utc::now();
    let (parts, _) = request.into_parts();
    let access = bearer(&parts, &state.verification, now)?;
    refuse_assisted_context(&parts)?;
    // The administrator check runs before the identifier is parsed, inside
    // the same transaction that reads the preview.
    let subject = target(&user_id);
    run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", PREVIEW_ROUTE, false),
        move |tx| {
            Box::pin(async move {
                require_admin(tx.actor())?;
                let subject = subject?;
                let actor = tx.actor().clone();
                preview(tx.connection(), &actor, subject).await
            })
        },
    )
    .await
    .map_err(authenticated_failure)
}

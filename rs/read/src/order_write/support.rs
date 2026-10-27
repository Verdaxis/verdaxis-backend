//! Administrator capability lifecycle and bounded support workspace reads.
//!
//! Every request is checked in the order Python applies: account state
//! (`get_current_user`), the feature switch and the administrator role
//! (`require_market_support_admin`), request validation, then the bootstrap
//! allowlist and the capability the action needs. Only then is a resource
//! read or written.
pub mod contexts;

use std::{collections::BTreeSet, sync::Arc};

use axum::body::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgRow};
use uuid::Uuid;

use super::effects::{AuditContext, MutationEffect, MutationKind};
use super::locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview};
use super::transaction::{
    BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
};
use super::{AuthenticatedActor, MarketErrorCode, MarketWriteError};
use crate::auth::{AccountState, AuthError, VerifiedAccess, validate_account};
use crate::market::live_benchmarks::live_benchmarks;
use crate::market::order_projection::PublicOrder;
use crate::market::terms::{validate_fame_schema, validate_lng_terms};
use crate::market::whitespace::is_python_whitespace;
use crate::market::windows::normalize_window;
use crate::routes::ports::pydantic_datetime;
use crate::{Config, MarketPolicy};

pub async fn run_support(
    pool: sqlx::PgPool,
    permits: Arc<tokio::sync::Semaphore>,
    command: SupportCommand,
) -> Result<Value, MarketWriteError> {
    let access = VerifiedAccess {
        subject: command.access.subject,
        issued_at: command.access.issued_at,
        issuance_out_of_range: command.access.issuance_out_of_range,
    };
    super::transaction::run_market_command(
        pool,
        permits,
        access,
        command,
        super::AdmissionClass::Execution,
    )
    .await
}

#[derive(Clone)]
pub struct SupportSettings {
    pub enabled: bool,
    bootstrap: BTreeSet<Uuid>,
    context_ttl_minutes: i64,
}
impl SupportSettings {
    pub fn new(enabled: bool, ids: &str) -> Self {
        Self {
            enabled,
            context_ttl_minutes: 60,
            bootstrap: ids
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect(),
        }
    }
    pub fn with_context_ttl_minutes(mut self, minutes: i64) -> Result<Self, MarketWriteError> {
        if !(1..=1440).contains(&minutes) {
            return Err(validation());
        }
        self.context_ttl_minutes = minutes;
        Ok(self)
    }
    pub fn context_ttl_minutes(&self) -> i64 {
        self.context_ttl_minutes
    }
    pub fn is_bootstrap(&self, actor: Uuid) -> bool {
        self.bootstrap.contains(&actor)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Capability {
    #[serde(rename = "MARKET_SUPPORT_LISTINGS")]
    Listings,
    #[serde(rename = "MARKET_SUPPORT_AUTHORIZATIONS")]
    Authorizations,
}
impl Capability {
    fn text(self) -> &'static str {
        match self {
            Self::Listings => "MARKET_SUPPORT_LISTINGS",
            Self::Authorizations => "MARKET_SUPPORT_AUTHORIZATIONS",
        }
    }
    fn from_text(text: &str) -> Option<Self> {
        match text {
            "MARKET_SUPPORT_LISTINGS" => Some(Self::Listings),
            "MARKET_SUPPORT_AUTHORIZATIONS" => Some(Self::Authorizations),
            _ => None,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct CapabilityGrant {
    pub user_id: Uuid,
    pub capability: Capability,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
}
impl CapabilityGrant {
    pub fn parse(value: Value, now: DateTime<Utc>) -> Result<Self, MarketWriteError> {
        let mut body: Self = serde_json::from_value(value).map_err(|_| validation())?;
        body.reason = reason(&body.reason)?;
        if body.expires_at.is_some_and(|at| at <= now) {
            return Err(validation());
        }
        Ok(body)
    }
}

/// Python's rule for a support reason: `Field(min_length=3, max_length=500)`
/// counts the text as sent, then the `normalize_reason` validator applies
/// `str.strip`. A reason of three spaces is therefore stored as `""`.
pub fn reason(raw: &str) -> Result<String, MarketWriteError> {
    if !(3..=500).contains(&raw.chars().count()) {
        return Err(validation());
    }
    Ok(raw.trim_matches(is_python_whitespace).to_owned())
}

#[derive(Clone, Debug)]
pub struct Page {
    pub skip: i64,
    pub limit: i64,
    pub query: Option<String>,
}
impl Page {
    pub fn parse(raw: Option<&str>) -> Result<Self, MarketWriteError> {
        let mut page = Self {
            skip: 0,
            limit: 25,
            query: None,
        };
        for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "skip" => page.skip = value.parse().map_err(|_| validation())?,
                "limit" => page.limit = value.parse().map_err(|_| validation())?,
                "query" => {
                    if value.chars().count() > 100 {
                        return Err(validation());
                    }
                    page.query = Some(value.trim_matches(is_python_whitespace).into());
                }
                _ => {}
            }
        }
        if page.skip < 0 || !(1..=100).contains(&page.limit) {
            return Err(validation());
        }
        Ok(page)
    }
}

/// The refusal for a request without valid access credentials.
pub fn missing_credentials() -> MarketWriteError {
    refusal(MarketErrorCode::AuthCredentialsInvalid)
}
fn validation() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
fn refusal(code: MarketErrorCode) -> MarketWriteError {
    MarketWriteError::new(code)
}
/// A stored row that the response types refuse is a server fault, as a
/// Python response-model failure is.
fn projection_fault() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}

/// The unparsed parts of one support request. Nothing here is decoded until
/// the caller is an approved administrator: Python resolves its
/// authentication dependencies before it validates a body, query or path.
pub enum SupportRequest {
    Capabilities,
    Grant {
        body: Bytes,
    },
    Revoke {
        assignment: String,
        body: Bytes,
    },
    Organizations {
        query: Option<String>,
    },
    Authorizations {
        organization: String,
        query: Option<String>,
    },
    Listings {
        organization: String,
        query: Option<String>,
    },
    Entry { organization: String },
    ContextStart { body: Bytes },
    ContextActive,
    ContextGet { context: String },
    ContextExit { context: String },
    Composite { organization: String },
    /// A legacy workspace mutation. Its body is never read.
    Retired,
}
impl SupportRequest {
    fn parse(&self, now: DateTime<Utc>) -> Result<SupportAction, MarketWriteError> {
        let action = match self {
            Self::Capabilities => SupportAction::Capabilities,
            Self::Grant { body } => {
                SupportAction::Grant(CapabilityGrant::parse(decode_json(body)?, now)?)
            }
            Self::Revoke { assignment, body } => {
                let assignment = identifier(assignment)?;
                let body = decode_json(body)?;
                let raw = body.get("reason").and_then(Value::as_str);
                let reason = reason(raw.ok_or_else(validation)?)?;
                SupportAction::Revoke { assignment, reason }
            }
            Self::Organizations { query } => {
                SupportAction::Organizations(Page::parse(query.as_deref())?)
            }
            Self::Authorizations {
                organization,
                query,
            } => SupportAction::Authorizations {
                organization: identifier(organization)?,
                page: Page::parse(query.as_deref())?,
            },
            Self::Listings {
                organization,
                query,
            } => SupportAction::Listings {
                organization: identifier(organization)?,
                page: Page::parse(query.as_deref())?,
            },
            Self::Entry { organization } => SupportAction::Entry(identifier(organization)?),
            Self::ContextStart { body } => SupportAction::ContextStart(contexts::ContextCreate::parse(decode_json(body)?)?),
            Self::ContextActive => SupportAction::ContextActive,
            Self::ContextGet { context } => SupportAction::ContextGet(identifier(context)?),
            Self::ContextExit { context } => SupportAction::ContextExit(identifier(context)?),
            Self::Composite { organization } => SupportAction::Composite(identifier(organization)?),
            Self::Retired => {
                return Err(refusal(MarketErrorCode::MarketSupportLegacyMutationRetired));
            }
        };
        Ok(action)
    }
}
fn decode_json(body: &[u8]) -> Result<Value, MarketWriteError> {
    serde_json::from_slice(body).map_err(|_| validation())
}
fn identifier(raw: &str) -> Result<Uuid, MarketWriteError> {
    raw.parse().map_err(|_| validation())
}

enum SupportAction {
    Capabilities,
    Grant(CapabilityGrant),
    Revoke { assignment: Uuid, reason: String },
    Entry(Uuid),
    ContextStart(contexts::ContextCreate),
    ContextActive,
    ContextGet(Uuid),
    ContextExit(Uuid),
    Composite(Uuid),
    Organizations(Page),
    Authorizations { organization: Uuid, page: Page },
    Listings { organization: Uuid, page: Page },
}
impl SupportAction {
    fn requires_bootstrap(&self) -> bool {
        matches!(self, Self::Grant(_) | Self::Revoke { .. })
    }
    /// Python `_lock_capability` for one organization resource and
    /// `_require_any_capability` for the organization list.
    fn permitted_by(&self, capabilities: &[Capability]) -> bool {
        match self {
            Self::Entry(_) | Self::ContextStart(_) | Self::ContextActive | Self::ContextGet(_) | Self::ContextExit(_) | Self::Composite(_) => capabilities.contains(&Capability::Authorizations) && capabilities.contains(&Capability::Listings),
            Self::Authorizations { .. } => capabilities.contains(&Capability::Authorizations),
            Self::Listings { .. } => capabilities.contains(&Capability::Listings),
            Self::Organizations(_) => !capabilities.is_empty(),
            Self::Capabilities | Self::Grant(_) | Self::Revoke { .. } => true,
        }
    }
}

/// A validated action and the caller's current capabilities.
struct Authorized {
    action: SupportAction,
    capabilities: Vec<Capability>,
}

pub struct SupportCommand {
    pub request: SupportRequest,
    pub settings: SupportSettings,
    pub access: Arc<VerifiedAccess>,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
    pub audit: AuditContext,
}

const ACCOUNT: &str = "SELECT role::text AS role, status::text AS status, \
     must_change_password, password_changed_at, authentication_revoked_at \
     FROM users WHERE id = $1";
/// Unrevoked, unexpired assignments. The catalogue lists them, and each
/// capability check reads them.
const CURRENT_CAPABILITIES: &str = "SELECT capability FROM staff_capability_assignments \
     WHERE user_id = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > $2) \
     ORDER BY capability";
const ACTIVE_CONTEXTS: &str = "SELECT id FROM market_support_contexts \
     WHERE actor_user_id = $1 AND status = 'ACTIVE' ORDER BY id";

impl SupportCommand {
    async fn authorize(
        &self,
        conn: &mut PgConnection,
        actor: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Authorized, MarketWriteError> {
        self.require_administrator(conn, actor).await?;
        let action = self.request.parse(now)?;
        if action.requires_bootstrap() && !self.settings.is_bootstrap(actor) {
            return Err(refusal(MarketErrorCode::SupportForbidden));
        }
        let capabilities = current_capabilities(conn, actor, now).await?;
        if !action.permitted_by(&capabilities) {
            return Err(refusal(MarketErrorCode::SupportCapabilityRequired));
        }
        Ok(Authorized {
            action,
            capabilities,
        })
    }

    /// `get_current_user` (token cutoffs, approval, forced password change),
    /// then `require_market_support_admin` (feature switch, then role).
    async fn require_administrator(
        &self,
        conn: &mut PgConnection,
        actor: Uuid,
    ) -> Result<(), MarketWriteError> {
        let row = sqlx::query(ACCOUNT)
            .bind(actor)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| refusal(MarketErrorCode::AuthCredentialsInvalid))?;
        let micros = |column: &str| -> Result<Option<i64>, MarketWriteError> {
            let at: Option<DateTime<Utc>> = row.try_get(column)?;
            Ok(at.map(|at| at.timestamp_micros()))
        };
        let account = AccountState {
            exists: true,
            status: row.try_get("status")?,
            must_change_password: row.try_get("must_change_password")?,
            password_changed_at: micros("password_changed_at")?,
            authentication_revoked_at: micros("authentication_revoked_at")?,
        };
        validate_account(&self.access, &account).map_err(account_refusal)?;
        if !self.settings.enabled {
            return Err(refusal(MarketErrorCode::SupportDisabled));
        }
        let role: Option<String> = row.try_get("role")?;
        if role.as_deref() != Some("ADMIN") {
            return Err(refusal(MarketErrorCode::SupportForbidden));
        }
        Ok(())
    }
}

/// Python answers an unapproved account or a required password change with
/// 403 before the feature switch. Token cutoffs and unknown accounts are 401.
fn account_refusal(error: AuthError) -> MarketWriteError {
    refusal(match error {
        AuthError::PasswordChanged => MarketErrorCode::AuthPasswordChanged,
        AuthError::SessionRevoked => MarketErrorCode::AuthSessionRevoked,
        AuthError::Pending | AuthError::Rejected | AuthError::PasswordChangeRequired => {
            MarketErrorCode::SupportForbidden
        }
        AuthError::MissingBearer | AuthError::Credentials => {
            MarketErrorCode::AuthCredentialsInvalid
        }
    })
}

async fn current_capabilities(
    conn: &mut PgConnection,
    actor: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<Capability>, MarketWriteError> {
    let names: Vec<String> = sqlx::query_scalar(CURRENT_CAPABILITIES)
        .bind(actor)
        .bind(now)
        .fetch_all(conn)
        .await?;
    Ok(names
        .iter()
        .filter_map(|name| Capability::from_text(name))
        .collect())
}

/// Grants lock the actor and target user rows, then the target's existing
/// assignment for the capability. A new grant serializes on the target row.
async fn grant_preview(
    conn: &mut PgConnection,
    actor: Uuid,
    grant: &CapabilityGrant,
) -> Result<MarketPreview, MarketWriteError> {
    let target_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
            .bind(grant.user_id)
            .fetch_one(&mut *conn)
            .await?;
    if !target_exists {
        return Err(refusal(MarketErrorCode::SupportTargetInvalid));
    }
    let assignments: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM staff_capability_assignments WHERE user_id = $1 AND capability = $2",
    )
    .bind(grant.user_id)
    .bind(grant.capability.text())
    .fetch_all(&mut *conn)
    .await?;
    let parties = PartyLockPreview::new([], [grant.user_id], actor, Value::Null);
    let mut locks = OperationLockPlan::new(MarketOperation::CapabilityGrant);
    locks
        .parties(&parties)?
        .resources(LockClass::Capability, assignments)?;
    Ok(MarketPreview { parties, locks })
}

/// Revocation locks the actor and assignment owner rows, the assignment, and
/// then every ACTIVE context of the assignment's user in id order. Python
/// locks the assignment before any context.
async fn revoke_preview(
    conn: &mut PgConnection,
    actor: Uuid,
    assignment: Uuid,
) -> Result<MarketPreview, MarketWriteError> {
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM staff_capability_assignments WHERE id = $1")
            .bind(assignment)
            .fetch_optional(&mut *conn)
            .await?;
    let contexts: Vec<Uuid> = match owner {
        Some(owner) => {
            sqlx::query_scalar(ACTIVE_CONTEXTS)
                .bind(owner)
                .fetch_all(&mut *conn)
                .await?
        }
        None => Vec::new(),
    };
    let fingerprint = json!({"target": owner, "contexts": &contexts});
    let parties = PartyLockPreview::new([], owner, actor, fingerprint);
    let mut locks = OperationLockPlan::new(MarketOperation::CapabilityRevoke);
    locks
        .parties(&parties)?
        .resources(LockClass::Capability, owner.map(|_| assignment))?
        .resources(LockClass::Context, contexts)?;
    Ok(MarketPreview { parties, locks })
}

/// Support reads lock only the actor row, as Python's `get_current_user`
/// does for these routes (`FOR UPDATE`: they are not in its shared-lock
/// list). The capability lock graph starts with exactly that user lock.
const SUPPORT_READ_LOCKS: MarketOperation = MarketOperation::CapabilityGrant;

fn read_preview(actor: Uuid) -> Result<MarketPreview, MarketWriteError> {
    let parties = PartyLockPreview::new([], [], actor, Value::Null);
    let mut locks = OperationLockPlan::new(SUPPORT_READ_LOCKS);
    locks.parties(&parties)?;
    Ok(MarketPreview { parties, locks })
}

impl MarketCommand for SupportCommand {
    type Output = Value;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let authorized = self.authorize(conn, actor, Utc::now()).await?;
            let preview = match &authorized.action {
                SupportAction::Grant(grant) => grant_preview(conn, actor, grant).await?,
                SupportAction::Revoke { assignment, .. } => {
                    revoke_preview(conn, actor, *assignment).await?
                }
                SupportAction::Entry(_) | SupportAction::ContextStart(_) | SupportAction::ContextActive | SupportAction::ContextGet(_) | SupportAction::ContextExit(_) | SupportAction::Composite(_) => contexts::preview(conn, actor, &authorized.action).await?,
                _ => read_preview(actor)?,
            };
            Ok(preview)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Value, MarketAttemptError>> {
        Box::pin(async move {
            let now = Utc::now();
            let actor = actor.user_id();
            let outcome = {
                let mut port = tx.support()?;
                let authorized = self.authorize(port.conn, actor, now).await?;
                let context = ActionContext {
                    actor,
                    now,
                    audit: &self.audit,
                    policy: &self.policy,
                    config: &self.config,
                    settings: &self.settings,
                };
                port.run(authorized, &context).await?
            };
            if !outcome.effects.is_empty() {
                let mut port = tx.effects()?;
                let mut set = port
                    .capture_before(&self.policy, &self.config, &[], now)
                    .await?
                    .stage();
                for effect in outcome.effects {
                    set.push(effect)?;
                }
                port.finish(&self.policy, &self.config, set).await?;
            }
            Ok(outcome.response)
        })
    }
}

/// What one locked action needs besides its database rows.
struct ActionContext<'a> {
    actor: Uuid,
    now: DateTime<Utc>,
    audit: &'a AuditContext,
    policy: &'a MarketPolicy,
    config: &'a Config,
    settings: &'a SupportSettings,
}

/// The response and the audit descriptors staged for one commit.
struct Outcome {
    response: Value,
    effects: Vec<MutationEffect>,
}
impl Outcome {
    fn read(response: Value) -> Self {
        Self {
            response,
            effects: Vec::new(),
        }
    }
}

const GRANT_TARGET: &str = "SELECT EXISTS(SELECT 1 FROM users \
     WHERE id = $1 AND role = 'ADMIN' AND status = 'APPROVED')";
const GRANT: &str = "INSERT INTO staff_capability_assignments \
     (id, user_id, capability, reason, granted_by_user_id, granted_at, expires_at) \
     VALUES ($1, $2, $3, $4, $5, $6, $7) \
     ON CONFLICT (user_id, capability) DO UPDATE SET reason = EXCLUDED.reason, \
     granted_by_user_id = EXCLUDED.granted_by_user_id, granted_at = EXCLUDED.granted_at, \
     expires_at = EXCLUDED.expires_at, revoked_at = NULL, revoked_by_user_id = NULL, \
     revocation_reason = NULL \
     RETURNING id, user_id, capability, reason, granted_by_user_id, granted_at, \
     expires_at, revoked_at, revoked_by_user_id, revocation_reason";
/// A repeated revocation keeps the first revocation's actor, time and reason.
const REVOKE: &str = "UPDATE staff_capability_assignments \
     SET revoked_at = COALESCE(revoked_at, $2), \
     revoked_by_user_id = CASE WHEN revoked_at IS NULL THEN $3 ELSE revoked_by_user_id END, \
     revocation_reason = CASE WHEN revoked_at IS NULL THEN $4 ELSE revocation_reason END \
     WHERE id = $1 \
     RETURNING id, user_id, capability, reason, granted_by_user_id, granted_at, \
     expires_at, revoked_at, revoked_by_user_id, revocation_reason";
const END_CONTEXT: &str = "UPDATE market_support_contexts \
     SET status = 'REVOKED', ended_at = $2, version = version + 1 \
     WHERE id = $1 AND status = 'ACTIVE'";

/// One filter for the organization count and page: approved REAL rows,
/// optionally matching a name or domain.
macro_rules! approved_real_organizations {
    () => {
        "FROM organizations WHERE verification_status = 'APPROVED' AND provenance = 'REAL' \
         AND ($1::text IS NULL OR name ILIKE $1 OR domain ILIKE $1)"
    };
}
const ORGANIZATION_COUNT: &str = concat!("SELECT count(*) ", approved_real_organizations!());
const ORGANIZATION_PAGE: &str = concat!(
    "SELECT jsonb_build_object('id', id, 'name', name, 'domain', domain, 'type', type) ",
    approved_real_organizations!(),
    " ORDER BY name, id OFFSET $2 LIMIT $3"
);

const AUTHORIZATION_COUNT: &str =
    "SELECT count(*) FROM market_support_authorizations WHERE organization_id = $1";
const AUTHORIZATION_PAGE: &str = "SELECT to_jsonb(a) AS stored, \
     quantity_mt::text AS quantity, price_per_mt_usd::text AS price, \
     carbon_intensity_gco2_mj::text AS carbon_intensity \
     FROM market_support_authorizations a WHERE organization_id = $1 \
     ORDER BY created_at DESC, id DESC OFFSET $2 LIMIT $3";
/// `AuthorizationResponse` fields copied from the stored row. The money
/// fields, the typed terms and the nested order are added separately.
const AUTHORIZATION_FIELDS: [&str; 24] = [
    "id",
    "organization_id",
    "accountable_user_id",
    "status",
    "product_id",
    "delivery_point_id",
    "availability_window",
    "authorization_expires_at",
    "order_expires_at",
    "terms_digest",
    "evidence_reference",
    "evidence_sha256",
    "commercial_consent_version",
    "commercial_consent_reference",
    "support_case_reference",
    "instruction_at",
    "acknowledge_exact_terms",
    "acknowledge_executable_standing_order",
    "created_by_actor_user_id",
    "created_at",
    "consumed_at",
    "revoked_at",
    "revoked_by_actor_user_id",
    "revocation_reason",
];
const AUTHORIZATION_TIMES: [&str; 6] = [
    "authorization_expires_at",
    "order_expires_at",
    "instruction_at",
    "created_at",
    "consumed_at",
    "revoked_at",
];
/// `OrderCreate` fields copied from the stored row.
const ORDER_FIELDS: [&str; 13] = [
    "product_id",
    "delivery_point_id",
    "certifications",
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "carbon_intensity_method",
    "feedstock",
    "origin",
    "off_spec",
    "off_spec_notes",
    "is_anonymous",
];

const LISTING_COUNT: &str = "SELECT count(*) FROM orderbook_orders \
     WHERE organization_id = $1 AND creation_method = 'MARKET_SUPPORT'";
const LISTING_COLUMNS: &str = "o.provenance::text AS provenance, o.side::text AS side, \
     o.price_per_mt_usd, o.expires_at, o.created_at, ''::text AS market_product, \
     COALESCE(p.name, '') AS product_name, COALESCE(p.fuel_type, '') AS product_fuel_type, \
     COALESCE(p.fuel_grade, 'Conventional') AS product_fuel_grade, \
     COALESCE(dp.name, '') AS delivery_point_name, \
     COALESCE(dp.region, '') AS delivery_point_region, \
     COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label, \
     o.owner_user_id, o.created_by_actor_user_id, o.support_authorization_id, \
     o.creation_method, o.version, o.id, o.product_id";
const LISTING_SOURCE: &str = "FROM orderbook_orders o \
     LEFT JOIN products p ON p.id = o.product_id \
     LEFT JOIN delivery_points dp ON dp.id = o.delivery_point_id \
     JOIN organizations org ON org.id = o.organization_id \
     WHERE o.organization_id = $1 AND o.creation_method = 'MARKET_SUPPORT' \
     ORDER BY o.created_at DESC, o.id DESC OFFSET $2 LIMIT $3";

pub(super) struct SupportPort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
}
impl<'a> SupportPort<'a> {
    pub(super) fn new(conn: &'a mut PgConnection, locked: &'a Locked) -> Self {
        Self { conn, locked }
    }

    async fn run(
        &mut self,
        authorized: Authorized,
        context: &ActionContext<'_>,
    ) -> Result<Outcome, MarketWriteError> {
        match authorized.action {
            SupportAction::Entry(organization) => self.entry(organization).await,
            SupportAction::ContextStart(body) => self.start_context(&body, context).await,
            SupportAction::ContextActive => self.active_context(context).await,
            SupportAction::ContextGet(id) => self.get_context(id, context).await,
            SupportAction::ContextExit(id) => self.exit_context(id, context).await,
            SupportAction::Composite(organization) => self.composite(organization, context).await,
            SupportAction::Capabilities => Ok(Outcome::read(json!(authorized.capabilities))),
            SupportAction::Grant(grant) => self.grant(&grant, context).await,
            SupportAction::Revoke { assignment, reason } => {
                self.revoke(assignment, &reason, context).await
            }
            SupportAction::Organizations(page) => {
                Ok(Outcome::read(self.organizations(&page).await?))
            }
            SupportAction::Authorizations { organization, page } => {
                self.require_real_organization(organization).await?;
                Ok(Outcome::read(
                    self.authorizations(organization, &page, context.now)
                        .await?,
                ))
            }
            SupportAction::Listings { organization, page } => {
                self.require_real_organization(organization).await?;
                Ok(Outcome::read(
                    self.listings(organization, &page, context).await?,
                ))
            }
        }
    }

    async fn grant(
        &mut self,
        grant: &CapabilityGrant,
        context: &ActionContext<'_>,
    ) -> Result<Outcome, MarketWriteError> {
        if !self.locked.contains(LockClass::User, grant.user_id) {
            return Err(refusal(MarketErrorCode::MarketSliceChanged));
        }
        let eligible: bool = sqlx::query_scalar(GRANT_TARGET)
            .bind(grant.user_id)
            .fetch_one(&mut *self.conn)
            .await?;
        if !eligible {
            return Err(refusal(MarketErrorCode::SupportTargetInvalid));
        }
        if grant.expires_at.is_some_and(|at| at <= context.now) {
            return Err(validation());
        }
        let row = sqlx::query(GRANT)
            .bind(Uuid::new_v4())
            .bind(grant.user_id)
            .bind(grant.capability.text())
            .bind(&grant.reason)
            .bind(context.actor)
            .bind(context.now)
            .bind(grant.expires_at)
            .fetch_one(&mut *self.conn)
            .await?;
        let changes = json!({
            "target_user_id": grant.user_id,
            "capability": grant.capability,
            "reason": grant.reason,
        });
        let effect = descriptor(
            MutationKind::SupportCapabilityGranted,
            row.try_get("id")?,
            "staff_capability_assignment",
            changes,
            context,
        );
        Ok(Outcome {
            response: assignment_response(&row)?,
            effects: vec![effect],
        })
    }

    async fn revoke(
        &mut self,
        assignment: Uuid,
        reason: &str,
        context: &ActionContext<'_>,
    ) -> Result<Outcome, MarketWriteError> {
        if !self.locked.contains(LockClass::Capability, assignment) {
            return Err(refusal(MarketErrorCode::SupportAssignmentNotFound));
        }
        let row = sqlx::query(REVOKE)
            .bind(assignment)
            .bind(context.now)
            .bind(context.actor)
            .bind(reason)
            .fetch_one(&mut *self.conn)
            .await?;
        let target: Uuid = row.try_get("user_id")?;
        let capability: String = row.try_get("capability")?;
        let mut effects = vec![descriptor(
            MutationKind::SupportCapabilityRevoked,
            assignment,
            "staff_capability_assignment",
            json!({"target_user_id": target, "capability": capability, "reason": reason}),
            context,
        )];
        let contexts: Vec<Uuid> = sqlx::query_scalar(ACTIVE_CONTEXTS)
            .bind(target)
            .fetch_all(&mut *self.conn)
            .await?;
        for support_context in contexts {
            if !self.locked.contains(LockClass::Context, support_context) {
                return Err(refusal(MarketErrorCode::MarketSliceChanged));
            }
            sqlx::query(END_CONTEXT)
                .bind(support_context)
                .bind(context.now)
                .execute(&mut *self.conn)
                .await?;
            effects.push(descriptor(
                MutationKind::SupportContextRevoked,
                support_context,
                "market_support_context",
                json!({
                    "status": "REVOKED",
                    "reason": reason,
                    "capability_assignment_id": assignment,
                }),
                context,
            ));
        }
        Ok(Outcome {
            response: assignment_response(&row)?,
            effects,
        })
    }

    async fn organizations(&mut self, page: &Page) -> Result<Value, MarketWriteError> {
        let pattern = page
            .query
            .as_ref()
            .filter(|query| !query.is_empty())
            .map(|query| format!("%{query}%"));
        let total: i64 = sqlx::query_scalar(ORGANIZATION_COUNT)
            .bind(&pattern)
            .fetch_one(&mut *self.conn)
            .await?;
        let items: Vec<Value> = sqlx::query_scalar(ORGANIZATION_PAGE)
            .bind(&pattern)
            .bind(page.skip)
            .bind(page.limit)
            .fetch_all(&mut *self.conn)
            .await?;
        Ok(page_response(items, total, page))
    }

    async fn require_real_organization(&mut self, id: Uuid) -> Result<(), MarketWriteError> {
        let row = sqlx::query(
            "SELECT verification_status, provenance::text AS provenance \
             FROM organizations WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or_else(|| refusal(MarketErrorCode::SupportOrganizationNotFound))?;
        // Python compares `str(status or "").upper()`.
        let status: Option<String> = row.try_get("verification_status")?;
        if status.unwrap_or_default().to_uppercase() != "APPROVED" {
            return Err(refusal(MarketErrorCode::SupportOrganizationNotApproved));
        }
        let provenance: String = row.try_get("provenance")?;
        if provenance != "REAL" {
            return Err(refusal(MarketErrorCode::SupportOrganizationNotReal));
        }
        Ok(())
    }

    async fn authorizations(
        &mut self,
        organization: Uuid,
        page: &Page,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        let total: i64 = sqlx::query_scalar(AUTHORIZATION_COUNT)
            .bind(organization)
            .fetch_one(&mut *self.conn)
            .await?;
        let rows = sqlx::query(AUTHORIZATION_PAGE)
            .bind(organization)
            .bind(page.skip)
            .bind(page.limit)
            .fetch_all(&mut *self.conn)
            .await?;
        let items = rows
            .iter()
            .map(|row| authorization_item(row, now))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page_response(items, total, page))
    }

    async fn listings(
        &mut self,
        organization: Uuid,
        page: &Page,
        context: &ActionContext<'_>,
    ) -> Result<Value, MarketWriteError> {
        let total: i64 = sqlx::query_scalar(LISTING_COUNT)
            .bind(organization)
            .fetch_one(&mut *self.conn)
            .await?;
        let digest = crate::market::visibility::private_digest_input_sql("o");
        let sql = format!("SELECT {digest}, {LISTING_COLUMNS} {LISTING_SOURCE}");
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(organization)
            .bind(page.skip)
            .bind(page.limit)
            .fetch_all(&mut *self.conn)
            .await?;
        let mut projections = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut projection = PublicOrder::from_row(row).map_err(|_| projection_fault())?;
            let product: Uuid = row.try_get("product_id")?;
            projection.labels["market_product"] = json!(
                context
                    .policy
                    .products
                    .iter()
                    .find(|candidate| candidate.id == product)
                    .map(|candidate| candidate.code.as_str())
            );
            // The same normalized key requests and finds each benchmark.
            let key = projection
                .benchmark_key(context.now)
                .map_err(|_| projection_fault())?;
            projections.push((row, projection, key));
        }
        let keys: Vec<_> = projections
            .iter()
            .filter_map(|(_, _, key)| key.clone())
            .collect();
        let mut nested = self.conn.begin().await?;
        let benchmarks = live_benchmarks(
            &mut nested,
            context.policy,
            context.config,
            &keys,
            context.now,
        )
        .await
        .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?;
        nested.commit().await?;
        let mut items = Vec::with_capacity(projections.len());
        for (row, projection, key) in projections {
            let benchmark = key.as_ref().and_then(|key| benchmarks.get(key));
            let order = projection
                .public(false, benchmark)
                .map_err(|_| projection_fault())?;
            items.push(listing_item(row, order)?);
        }
        Ok(page_response(items, total, page))
    }
}

fn page_response(items: Vec<Value>, total: i64, page: &Page) -> Value {
    json!({"items": items, "total": total, "skip": page.skip, "limit": page.limit})
}

/// `CapabilityAssignmentResponse`, field by field.
fn assignment_response(row: &PgRow) -> Result<Value, MarketWriteError> {
    let time = |column: &str| -> Result<Option<String>, MarketWriteError> {
        let at: Option<DateTime<Utc>> = row.try_get(column)?;
        Ok(at.map(pydantic_datetime))
    };
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?,
        "user_id": row.try_get::<Uuid, _>("user_id")?,
        "capability": row.try_get::<String, _>("capability")?,
        "reason": row.try_get::<String, _>("reason")?,
        "granted_by_user_id": row.try_get::<Uuid, _>("granted_by_user_id")?,
        "granted_at": time("granted_at")?,
        "expires_at": time("expires_at")?,
        "revoked_at": time("revoked_at")?,
        "revoked_by_user_id": row.try_get::<Option<Uuid>, _>("revoked_by_user_id")?,
        "revocation_reason": row.try_get::<Option<String>, _>("revocation_reason")?,
    }))
}

/// `AuthorizationResponse`: the stored fields, the exact money text, the
/// typed fuel terms and the nested `OrderCreate`.
fn authorization_item(row: &PgRow, now: DateTime<Utc>) -> Result<Value, MarketWriteError> {
    let stored: Value = row.try_get("stored")?;
    let quantity = json!(row.try_get::<String, _>("quantity")?);
    let price = json!(row.try_get::<String, _>("price")?);
    let fame_terms = typed_terms(TermsKind::Fame, &stored["order_side"], &stored["fame_terms"])?;
    let lng_terms = typed_terms(TermsKind::Lng, &stored["order_side"], &stored["lng_terms"])?;
    let mut item = Map::new();
    for field in AUTHORIZATION_FIELDS {
        item.insert(field.into(), stored[field].clone());
    }
    for field in AUTHORIZATION_TIMES {
        item.insert(field.into(), response_time(&stored[field])?);
    }
    item.insert("quantity_mt".into(), quantity.clone());
    item.insert("price_per_mt_usd".into(), price.clone());
    item.insert("fame_terms".into(), fame_terms.clone());
    item.insert("lng_terms".into(), lng_terms.clone());

    let mut order = Map::new();
    for field in ORDER_FIELDS {
        order.insert(field.into(), stored[field].clone());
    }
    let window = stored["availability_window"].as_str().unwrap_or_default();
    let window = normalize_window(window, now.date_naive()).map_err(|_| projection_fault())?;
    order.insert("side".into(), stored["order_side"].clone());
    order.insert("availability_window".into(), json!(window));
    order.insert("quantity_mt".into(), quantity);
    order.insert("price_per_mt_usd".into(), price);
    order.insert(
        "carbon_intensity_gco2_mj".into(),
        json!(row.try_get::<Option<String>, _>("carbon_intensity")?),
    );
    order.insert(
        "expires_at".into(),
        response_time(&stored["order_expires_at"])?,
    );
    order.insert("port_id".into(), Value::Null);
    order.insert("vessel_id".into(), Value::Null);
    // The final confirmation is transient evidence and is never stored.
    order.insert("support_confirmation".into(), Value::Null);
    order.insert("fame_terms".into(), fame_terms);
    order.insert("lng_terms".into(), lng_terms);
    item.insert("order".into(), Value::Object(order));
    Ok(Value::Object(item))
}

#[derive(Clone, Copy)]
enum TermsKind {
    Fame,
    Lng,
}

/// Stored declarations pass the typed schema (`FameOrderTerms` or
/// `LngOrderTerms`) before they are returned. Python stores each declaration
/// as that schema's JSON dump, so a valid value is returned as stored.
fn typed_terms(kind: TermsKind, side: &Value, stored: &Value) -> Result<Value, MarketWriteError> {
    if stored.is_null() {
        return Ok(Value::Null);
    }
    let side = side.as_str().unwrap_or_default();
    let checked = match kind {
        TermsKind::Fame => validate_fame_schema(side, Some(stored)),
        TermsKind::Lng => validate_lng_terms(side, Some(stored)),
    };
    checked.map_err(|_| projection_fault())?;
    Ok(stored.clone())
}

/// A stored timestamp in the response format the other Rust routes use.
fn response_time(stored: &Value) -> Result<Value, MarketWriteError> {
    let Some(text) = stored.as_str() else {
        return Ok(stored.clone());
    };
    let at = DateTime::parse_from_rfc3339(text).map_err(|_| projection_fault())?;
    Ok(json!(pydantic_datetime(at.with_timezone(&Utc))))
}

/// `AssistedListingResponse`.
fn listing_item(row: &PgRow, order: Value) -> Result<Value, MarketWriteError> {
    let id: Uuid = row.try_get("id")?;
    let version: i32 = row.try_get("version")?;
    Ok(json!({
        "order": order,
        "accountable_user_id": row.try_get::<Option<Uuid>, _>("owner_user_id")?,
        "created_by_actor_user_id": row.try_get::<Option<Uuid>, _>("created_by_actor_user_id")?,
        "creation_method": row.try_get::<String, _>("creation_method")?,
        "support_authorization_id": row.try_get::<Option<Uuid>, _>("support_authorization_id")?,
        "version": version,
        "etag": format!("W/\"order-{id}-v{version}\""),
    }))
}

fn descriptor(
    kind: MutationKind,
    id: Uuid,
    resource: &str,
    changes: Value,
    context: &ActionContext<'_>,
) -> MutationEffect {
    MutationEffect {
        kind,
        actor_id: context.actor,
        organization_id: Uuid::nil(),
        resource_type: resource.into(),
        resource_id: id,
        changes,
        event: None,
        at: context.now,
        audit_context: context.audit.clone(),
    }
}

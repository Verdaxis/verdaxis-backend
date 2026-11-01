//! Administrator obligation list and responsible-person reassignment.
//!
//! When a person leaves a company, the company's confirmed and delivered
//! trades stay in force. At a verified company request (the opaque case
//! reference is the record of that authority) an administrator can name a
//! current member of the same recorded organization, with the role the trade
//! side requires (BUYER for the buyer side, SUPPLIER for the seller side), as
//! the person responsible for one trade side. Only that side's
//! `buyer_user_id` or `seller_user_id` changes: quantities, prices, fees,
//! status, order authorship, receipts and earlier history stay as they are.
//!
//! Only a departed person is replaced: the current person must no longer be
//! a member of the side's recorded organization (detached or moved). A
//! rejected person who is still attached keeps the organization's settlement
//! rights and is still a member.
//!
//! Locks follow the declared graph: the recorded organization, then the
//! sorted users (administrator, current and replacement person), then the
//! trade. Under the locks the command revalidates the side, the trade status,
//! an active freeze, the current person's membership and the replacement's
//! membership. A side whose linked order was entered by market support is
//! refused: both settlement paths require that side's person to equal the
//! order owner and creator, so it needs a separately reviewed support
//! resolution.
//!
//! The command's audit record holds the old and new person, the administrator,
//! the opaque case reference and the time. Its market event goes to the
//! recorded organization only and names no person, case or reason.

use axum::body::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::authority::ActorRole;
use super::effects::AuditContext;
use super::locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview};
use super::transaction::{
    BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
};
use super::{AuthenticatedActor, MarketErrorCode, MarketWriteError};

/// The audit action of one completed reassignment.
pub const AUDIT_ACTION: &str = "trade.responsibility_reassigned";

/// The market event type sent to the recorded organization.
pub const EVENT_TYPE: &str = "trade_responsibility_reassigned";

/// The largest request body. The four fields fit well inside it, so a larger
/// body can only carry text the contract does not accept.
pub const REQUEST_BODY_LIMIT_BYTES: usize = 4096;

/// The largest obligation page, and the page size when none is given.
pub const OBLIGATION_PAGE_LIMIT: i64 = 100;

/// The longest case reference. It matches the account offboarding contract.
pub const CASE_REFERENCE_MAX_LENGTH: usize = 64;

/// The ticket systems a case reference may name.
pub const CASE_REFERENCE_PREFIXES: [&str; 4] = ["CASE", "REQ", "SEC", "TICKET"];

/// At most this many digit groups follow the prefix.
const CASE_REFERENCE_MAX_GROUPS: usize = 3;

/// At most this many digits form one group.
const CASE_REFERENCE_MAX_GROUP_DIGITS: usize = 12;

/// The trade states whose obligations a responsible person still carries.
const REASSIGNABLE_STATUSES: [&str; 2] = ["CONFIRMED", "DELIVERED"];

fn validation() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}

fn refusal(code: MarketErrorCode) -> MarketWriteError {
    MarketWriteError::new(code)
}

// ---------------------------------------------------------------------------
// Request contract.
// ---------------------------------------------------------------------------

/// The trade side whose responsible person changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResponsibleSide {
    Buyer,
    Seller,
}

impl ResponsibleSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Seller => "SELLER",
        }
    }

    /// The account role both settlement paths require on this side: the
    /// seller side must be a supplier, because only a supplier can pay.
    pub fn required_role(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Seller => "SUPPLIER",
        }
    }
}

/// An opaque reference to the verified company request, kept outside Verdaxis.
///
/// The grammar is `PREFIX-DIGITS`, with `PREFIX` one of
/// [`CASE_REFERENCE_PREFIXES`] and then one to three groups of 1 to 12 digits
/// separated by `-` or `/`, for example `CASE-2026/0042`. At most
/// [`CASE_REFERENCE_MAX_LENGTH`] characters. Only digits follow the prefix, so
/// the field cannot carry a name, an email address or a narrative.
#[derive(Clone, PartialEq, Eq)]
pub struct CaseReference(String);

impl CaseReference {
    pub fn parse(value: &str) -> Result<Self, MarketWriteError> {
        if value.len() > CASE_REFERENCE_MAX_LENGTH {
            return Err(validation());
        }
        let (prefix, identifier) = value.split_once('-').ok_or_else(validation)?;
        if !CASE_REFERENCE_PREFIXES.contains(&prefix) {
            return Err(validation());
        }
        let groups: Vec<&str> = identifier.split(['-', '/']).collect();
        let valid_groups = groups.len() <= CASE_REFERENCE_MAX_GROUPS
            && groups.iter().all(|group| {
                (1..=CASE_REFERENCE_MAX_GROUP_DIGITS).contains(&group.len())
                    && group.bytes().all(|byte| byte.is_ascii_digit())
            });
        if !valid_groups {
            return Err(validation());
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The reference is never written to a log line.
impl std::fmt::Debug for CaseReference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CaseReference(REDACTED)")
    }
}

/// The JSON body exactly as the contract states it: four required fields and
/// nothing else, so no free text can enter the record.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReassignBody {
    side: ResponsibleSide,
    replacement_user_id: Uuid,
    evidence_case_ref: String,
    expected_current_user_id: Uuid,
}

/// The validated body of one reassignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReassignRequest {
    pub side: ResponsibleSide,
    pub replacement_user_id: Uuid,
    pub evidence_case_ref: CaseReference,
    /// The person the administrator saw on the side. A different current
    /// person makes the command refuse instead of overwriting newer work.
    pub expected_current_user_id: Uuid,
}

impl ReassignRequest {
    pub fn parse(body: &[u8]) -> Result<Self, MarketWriteError> {
        if body.len() > REQUEST_BODY_LIMIT_BYTES {
            return Err(validation());
        }
        let raw: ReassignBody = serde_json::from_slice(body).map_err(|_| validation())?;
        if raw.replacement_user_id == raw.expected_current_user_id {
            return Err(validation());
        }
        Ok(Self {
            side: raw.side,
            replacement_user_id: raw.replacement_user_id,
            evidence_case_ref: CaseReference::parse(&raw.evidence_case_ref)?,
            expected_current_user_id: raw.expected_current_user_id,
        })
    }
}

/// One page of a person's obligation list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObligationPage {
    pub skip: i64,
    pub limit: i64,
}

impl ObligationPage {
    pub fn parse(query: Option<&str>) -> Result<Self, MarketWriteError> {
        let mut page = Self {
            skip: 0,
            limit: OBLIGATION_PAGE_LIMIT,
        };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "skip" => page.skip = value.parse().map_err(|_| validation())?,
                "limit" => page.limit = value.parse().map_err(|_| validation())?,
                _ => {}
            }
        }
        if page.skip < 0 || !(1..=OBLIGATION_PAGE_LIMIT).contains(&page.limit) {
            return Err(validation());
        }
        Ok(page)
    }
}

// ---------------------------------------------------------------------------
// Sealed command.
// ---------------------------------------------------------------------------

pub async fn execute_reassignment(
    pool: sqlx::PgPool,
    permits: std::sync::Arc<tokio::sync::Semaphore>,
    access: crate::auth::VerifiedAccess,
    command: ReassignmentCommand,
) -> Result<ReassignmentResult, MarketWriteError> {
    super::transaction::run_market_command(
        pool,
        permits,
        access,
        command,
        super::AdmissionClass::Execution,
    )
    .await
}

pub enum ReassignmentInput {
    List {
        subject: String,
        query: Option<String>,
    },
    Reassign {
        trade: String,
        body: Bytes,
    },
}

pub struct ReassignmentCommand {
    pub input: ReassignmentInput,
    pub audit: AuditContext,
}

pub struct ReassignmentResult {
    pub status: u16,
    pub body: Value,
}

/// The lock identities one reassignment needs, read before any lock.
struct Discovery {
    organizations: Vec<Uuid>,
    users: Vec<Uuid>,
    trade: Option<Uuid>,
    fingerprint: Value,
}

/// Read-only discovery. It never refuses: every refusal happens under locks,
/// after the administrator check, so a caller learns nothing from discovery.
/// Only a current administrator with a well-formed request gets a lock plan
/// beyond the own user row.
async fn discover(
    conn: &mut PgConnection,
    actor: Uuid,
    trade: &str,
    body: &Bytes,
) -> Result<Option<Discovery>, MarketAttemptError> {
    let (Ok(trade), Ok(request)) = (trade.parse::<Uuid>(), ReassignRequest::parse(body)) else {
        return Ok(None);
    };
    let role: Option<String> = sqlx::query_scalar("SELECT role::text FROM users WHERE id=$1")
        .bind(actor)
        .fetch_optional(&mut *conn)
        .await?;
    if role.as_deref() != Some("ADMIN") {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT buyer_id,seller_id,buyer_user_id,seller_user_id FROM trades WHERE id=$1",
    )
    .bind(trade)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(Some(Discovery {
            organizations: Vec::new(),
            users: Vec::new(),
            trade: None,
            fingerprint: json!({"trade": null}),
        }));
    };
    let (organization, current): (Uuid, Option<Uuid>) = match request.side {
        ResponsibleSide::Buyer => (row.try_get("buyer_id")?, row.try_get("buyer_user_id")?),
        ResponsibleSide::Seller => (row.try_get("seller_id")?, row.try_get("seller_user_id")?),
    };
    let replacement_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
            .bind(request.replacement_user_id)
            .fetch_one(&mut *conn)
            .await?;
    let replacement = replacement_exists.then_some(request.replacement_user_id);
    Ok(Some(Discovery {
        organizations: vec![organization],
        users: current.into_iter().chain(replacement).collect(),
        trade: Some(trade),
        fingerprint: json!({
            "trade": trade,
            "organization": organization,
            "current": current,
            "replacement": replacement,
        }),
    }))
}

impl MarketCommand for ReassignmentCommand {
    type Output = ReassignmentResult;

    /// A row that disappears before its lock means the discovery is out of
    /// date: the owner plans again within its attempt bound.
    fn replans_missing_lock_rows(&self) -> bool {
        true
    }

    fn prepare_response(&self, value: &Self::Output) -> Result<(), MarketAttemptError> {
        serde_json::to_vec(&value.body)
            .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?;
        Ok(())
    }

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let discovery = match &self.input {
                ReassignmentInput::List { .. } => None,
                ReassignmentInput::Reassign { trade, body } => {
                    discover(conn, actor, trade, body).await?
                }
            };
            let Discovery {
                organizations,
                users,
                trade,
                fingerprint,
            } = discovery.unwrap_or(Discovery {
                organizations: Vec::new(),
                users: Vec::new(),
                trade: None,
                fingerprint: json!({"actor": actor}),
            });
            let parties = PartyLockPreview::new(organizations, users, actor, fingerprint);
            let mut locks = OperationLockPlan::new(MarketOperation::Reassign);
            locks.parties(&parties)?;
            if let Some(trade) = trade {
                locks.trade(trade)?;
            }
            Ok(MarketPreview { parties, locks })
        })
    }

    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            // Authority first, then request validation, then resource lookup.
            require_admin(actor)?;
            match &self.input {
                ReassignmentInput::List { subject, query } => {
                    let subject = subject.parse().map_err(|_| validation())?;
                    let page = ObligationPage::parse(query.as_deref())?;
                    let body = tx.reassignment(actor)?.list(subject, &page).await?;
                    Ok(ReassignmentResult { status: 200, body })
                }
                ReassignmentInput::Reassign { trade, body } => {
                    let trade = trade.parse().map_err(|_| validation())?;
                    let request = ReassignRequest::parse(body)?;
                    let body = tx
                        .reassignment(actor)?
                        .reassign(trade, &request, &self.audit)
                        .await?;
                    Ok(ReassignmentResult { status: 200, body })
                }
            }
        })
    }
}

fn require_admin(actor: &AuthenticatedActor) -> Result<(), MarketWriteError> {
    if actor.account().role != ActorRole::Admin {
        return Err(refusal(MarketErrorCode::SupportForbidden));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Locked port.
// ---------------------------------------------------------------------------

/// The trade side facts the command reads under the trade lock.
struct SideFacts {
    status: String,
    organization: Uuid,
    current: Option<Uuid>,
    /// The person recorded on the other side. The replacement cannot be them.
    counterparty: Option<Uuid>,
    linked_order: Option<Uuid>,
}

pub(super) struct ReassignmentPort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
    actor: &'a AuthenticatedActor,
}

impl<'a> ReassignmentPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &'a Locked,
        actor: &'a AuthenticatedActor,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        require_admin(actor)?;
        if !locked.contains(LockClass::User, actor.user_id()) {
            return Err(refusal(MarketErrorCode::MarketSliceChanged));
        }
        Ok(Self {
            conn,
            locked,
            actor,
        })
    }

    /// The person's confirmed and delivered trade sides, newest first, with
    /// the freeze and assisted-side indicators the administrator needs.
    async fn list(
        &mut self,
        subject: Uuid,
        page: &ObligationPage,
    ) -> Result<Value, MarketWriteError> {
        let known: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
            .bind(subject)
            .fetch_one(&mut *self.conn)
            .await?;
        if !known {
            return Err(refusal(MarketErrorCode::UserNotFound));
        }
        // Count and page share one statement snapshot.
        let row = sqlx::query(
            "WITH sides AS MATERIALIZED (\
               SELECT t.id,'BUYER' AS side,t.status::text AS status,t.buyer_id AS organization_id,t.buyer_user_id AS user_id,t.bid_order_id AS order_id,t.created_at \
               FROM trades t WHERE t.buyer_user_id=$1 AND t.status::text IN ('CONFIRMED','DELIVERED') \
               UNION ALL \
               SELECT t.id,'SELLER',t.status::text,t.seller_id,t.seller_user_id,t.ask_order_id,t.created_at \
               FROM trades t WHERE t.seller_user_id=$1 AND t.status::text IN ('CONFIRMED','DELIVERED')), \
             page AS (SELECT * FROM sides ORDER BY created_at DESC,id DESC,side OFFSET $2 LIMIT $3) \
             SELECT (SELECT count(*) FROM sides) AS total,COALESCE((SELECT jsonb_agg(jsonb_build_object(\
               'trade_id',p.id,'side',p.side,'status',p.status,\
               'recorded_organization_id',p.organization_id,'responsible_user_id',p.user_id,\
               'frozen',EXISTS(SELECT 1 FROM trade_freezes f WHERE f.trade_id=p.id AND f.status='ACTIVE'),\
               'manual_resolution_required',COALESCE((SELECT o.creation_method::text='MARKET_SUPPORT' FROM orderbook_orders o WHERE o.id=p.order_id),false)\
             ) ORDER BY p.created_at DESC,p.id DESC,p.side) FROM page p),'[]'::jsonb) AS items",
        )
        .bind(subject)
        .bind(page.skip)
        .bind(page.limit)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(json!({
            "user_id": subject,
            "items": row.try_get::<Value, _>("items")?,
            "total": row.try_get::<i64, _>("total")?,
            "skip": page.skip,
            "limit": page.limit,
        }))
    }

    async fn side(
        &mut self,
        trade: Uuid,
        side: ResponsibleSide,
    ) -> Result<SideFacts, MarketWriteError> {
        let row = sqlx::query("SELECT status::text AS status,buyer_id,seller_id,buyer_user_id,seller_user_id,bid_order_id,ask_order_id FROM trades WHERE id=$1")
            .bind(trade)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or_else(|| refusal(MarketErrorCode::TradeNotFound))?;
        if !self.locked.contains(LockClass::Trade, trade) {
            return Err(refusal(MarketErrorCode::MarketSliceChanged));
        }
        let (organization, current, counterparty, linked_order) = match side {
            ResponsibleSide::Buyer => (
                row.try_get("buyer_id")?,
                row.try_get("buyer_user_id")?,
                row.try_get("seller_user_id")?,
                row.try_get("bid_order_id")?,
            ),
            ResponsibleSide::Seller => (
                row.try_get("seller_id")?,
                row.try_get("seller_user_id")?,
                row.try_get("buyer_user_id")?,
                row.try_get("ask_order_id")?,
            ),
        };
        if !self.locked.contains(LockClass::Organization, organization) {
            return Err(refusal(MarketErrorCode::MarketSliceChanged));
        }
        Ok(SideFacts {
            status: row.try_get("status")?,
            organization,
            current,
            counterparty,
            linked_order,
        })
    }

    /// Both settlement paths bind a market-support side to its order owner and
    /// creator, so only an ordinary side may change its person.
    async fn require_ordinary_side(&mut self, order: Option<Uuid>) -> Result<(), MarketWriteError> {
        let Some(order) = order else {
            return Ok(());
        };
        let method: Option<String> =
            sqlx::query_scalar("SELECT creation_method::text FROM orderbook_orders WHERE id=$1")
                .bind(order)
                .fetch_optional(&mut *self.conn)
                .await?;
        if method.as_deref() == Some("MARKET_SUPPORT") {
            return Err(refusal(
                MarketErrorCode::ReassignmentManualResolutionRequired,
            ));
        }
        Ok(())
    }

    /// Only a departed person is replaced. The person's row is locked, so a
    /// person who rejoins waits for this transaction or is seen here.
    async fn require_departed(
        &mut self,
        facts: &SideFacts,
        current: Uuid,
    ) -> Result<(), MarketWriteError> {
        if !self.locked.contains(LockClass::User, current) {
            return Err(refusal(MarketErrorCode::MarketSliceChanged));
        }
        let organization: Option<Uuid> =
            sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                .bind(current)
                .fetch_one(&mut *self.conn)
                .await?;
        if organization == Some(facts.organization) {
            return Err(refusal(MarketErrorCode::ReassignmentCurrentStillMember));
        }
        Ok(())
    }

    /// The replacement is a current, approved member of the recorded
    /// organization with the role the side requires, and is not the person on
    /// the other side.
    async fn require_eligible_replacement(
        &mut self,
        facts: &SideFacts,
        side: ResponsibleSide,
        replacement: Uuid,
    ) -> Result<(), MarketWriteError> {
        let ineligible = || refusal(MarketErrorCode::ReassignmentReplacementIneligible);
        if !self.locked.contains(LockClass::User, replacement)
            || facts.counterparty == Some(replacement)
        {
            return Err(ineligible());
        }
        let row = sqlx::query("SELECT organization_id,role::text AS role,status::text AS status,email_verified,must_change_password FROM users WHERE id=$1")
            .bind(replacement)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or_else(ineligible)?;
        let role: String = row.try_get("role")?;
        let eligible = row.try_get::<Option<Uuid>, _>("organization_id")?
            == Some(facts.organization)
            && role == side.required_role()
            && row.try_get::<String, _>("status")? == "APPROVED"
            && row.try_get::<bool, _>("email_verified")?
            && !row.try_get::<bool, _>("must_change_password")?;
        if !eligible {
            return Err(ineligible());
        }
        Ok(())
    }

    async fn reassign(
        &mut self,
        trade: Uuid,
        request: &ReassignRequest,
        audit: &AuditContext,
    ) -> Result<Value, MarketWriteError> {
        let facts = self.side(trade, request.side).await?;
        let current = facts
            .current
            .filter(|current| *current == request.expected_current_user_id)
            .ok_or_else(|| refusal(MarketErrorCode::ReassignmentStale))?;
        if !REASSIGNABLE_STATUSES.contains(&facts.status.as_str()) {
            return Err(refusal(MarketErrorCode::TradeNotReassignable));
        }
        // A freeze is never overridden: the trade lock serializes this check
        // with freeze administration through commit.
        super::freeze::ensure_settleable(self.conn, self.locked, trade).await?;
        self.require_ordinary_side(facts.linked_order).await?;
        self.require_departed(&facts, current).await?;
        let replacement = request.replacement_user_id;
        self.require_eligible_replacement(&facts, request.side, replacement)
            .await?;

        let update = match request.side {
            ResponsibleSide::Buyer => {
                "UPDATE trades SET buyer_user_id=$2 WHERE id=$1 AND buyer_user_id=$3"
            }
            ResponsibleSide::Seller => {
                "UPDATE trades SET seller_user_id=$2 WHERE id=$1 AND seller_user_id=$3"
            }
        };
        let changed = sqlx::query(update)
            .bind(trade)
            .bind(replacement)
            .bind(current)
            .execute(&mut *self.conn)
            .await?
            .rows_affected();
        if changed != 1 {
            return Err(refusal(MarketErrorCode::ReassignmentStale));
        }
        let at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *self.conn)
            .await?;
        let changes = json!({
            "side": request.side,
            "organization_id": facts.organization,
            "trade_status": facts.status,
            "responsible_user_id": {"before": current, "after": replacement},
            "evidence_case_ref": request.evidence_case_ref.as_str(),
        });
        super::effects::append_administrator_audit(
            self.conn,
            self.actor.user_id(),
            AUDIT_ACTION,
            trade,
            changes,
            at,
            audit,
        )
        .await?;
        self.append_event(trade, request.side, facts.organization, at)
            .await?;
        Ok(json!({
            "trade_id": trade,
            "side": request.side,
            "status": facts.status,
            "recorded_organization_id": facts.organization,
            "previous_responsible_user_id": current,
            "responsible_user_id": replacement,
            "reassigned_at": at,
        }))
    }

    /// Tell the recorded organization's own streams to reload the trade. The
    /// payload names no person, case or reason, and the counterparty is not a
    /// participant.
    async fn append_event(
        &mut self,
        trade: Uuid,
        side: ResponsibleSide,
        organization: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at) VALUES($1,$2,'trade',$3,$4::json,$5::json,$6)")
            .bind(Uuid::new_v4())
            .bind(EVENT_TYPE)
            .bind(trade.to_string())
            .bind(json!([organization]))
            .bind(json!({"id": trade, "side": side}))
            .bind(at)
            .execute(&mut *self.conn)
            .await?;
        super::transaction::wake_market_event_dispatchers(self.conn).await?;
        Ok(())
    }
}

//! Administrator freeze history. Trade status, orders and stock remain unchanged.

use axum::body::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row, postgres::PgRow};
use uuid::Uuid;

use super::authority::ActorRole;
use super::effects::AuditContext;
use super::locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview};
use super::transaction::{
    BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
};
use super::{AuthenticatedActor, MarketErrorCode, MarketWriteError};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FreezeReason {
    PaymentDispute,
    DeliveryDispute,
    TermsDispute,
    ComplianceReview,
    FraudReview,
    CourtOrder,
    Other,
}
impl FreezeReason {
    fn text(self) -> &'static str {
        match self {
            Self::PaymentDispute => "PAYMENT_DISPUTE",
            Self::DeliveryDispute => "DELIVERY_DISPUTE",
            Self::TermsDispute => "TERMS_DISPUTE",
            Self::ComplianceReview => "COMPLIANCE_REVIEW",
            Self::FraudReview => "FRAUD_REVIEW",
            Self::CourtOrder => "COURT_ORDER",
            Self::Other => "OTHER",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct FreezeRequest {
    pub reason_code: FreezeReason,
    pub note: String,
}
impl FreezeRequest {
    pub fn parse(value: &Value) -> Result<Self, MarketWriteError> {
        let mut request: Self = serde_json::from_value(value.clone()).map_err(|_| validation())?;
        request.note = note(&request.note)?;
        Ok(request)
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct UnfreezeRequest {
    pub note: String,
}
impl UnfreezeRequest {
    pub fn parse(value: &Value) -> Result<Self, MarketWriteError> {
        let mut request: Self = serde_json::from_value(value.clone()).map_err(|_| validation())?;
        request.note = note(&request.note)?;
        Ok(request)
    }
}
fn note(raw: &str) -> Result<String, MarketWriteError> {
    let value = raw.trim();
    if !(3..=500).contains(&value.chars().count()) {
        return Err(validation());
    }
    Ok(value.to_owned())
}
fn validation() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenPage {
    pub skip: i64,
    pub limit: i64,
}
impl FrozenPage {
    pub fn parse(query: Option<&str>) -> Result<Self, MarketWriteError> {
        let mut page = Self { skip: 0, limit: 25 };
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "skip" => page.skip = value.parse().map_err(|_| validation())?,
                "limit" => page.limit = value.parse().map_err(|_| validation())?,
                _ => {}
            }
        }
        if page.skip < 0 || !(1..=100).contains(&page.limit) {
            return Err(validation());
        }
        Ok(page)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FreezeResponse {
    pub id: Uuid,
    pub trade_id: Uuid,
    pub status: String,
    pub reason_code: FreezeReason,
    pub note: String,
    pub frozen_by_user_id: Uuid,
    pub frozen_at: DateTime<Utc>,
    pub cleared_by_user_id: Option<Uuid>,
    pub cleared_at: Option<DateTime<Utc>>,
    pub clear_note: Option<String>,
    pub version: i32,
}
fn project(row: &PgRow) -> Result<FreezeResponse, MarketWriteError> {
    let reason: String = row.try_get("reason_code")?;
    Ok(FreezeResponse {
        id: row.try_get("id")?,
        trade_id: row.try_get("trade_id")?,
        status: row.try_get("status")?,
        reason_code: serde_json::from_value(Value::String(reason))
            .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?,
        note: row.try_get("note")?,
        frozen_by_user_id: row.try_get("frozen_by_user_id")?,
        frozen_at: row.try_get("frozen_at")?,
        cleared_by_user_id: row.try_get("cleared_by_user_id")?,
        cleared_at: row.try_get("cleared_at")?,
        clear_note: row.try_get("clear_note")?,
        version: row.try_get("version")?,
    })
}

pub async fn execute_freeze(
    pool: sqlx::PgPool,
    permits: std::sync::Arc<tokio::sync::Semaphore>,
    access: crate::auth::VerifiedAccess,
    command: FreezeCommand,
) -> Result<FreezeResult, MarketWriteError> {
    super::transaction::run_market_command(
        pool,
        permits,
        access,
        command,
        super::AdmissionClass::Execution,
    )
    .await
}

pub enum FreezeInput {
    Freeze { trade: String, body: Bytes },
    Unfreeze { trade: String, body: Bytes },
    List { query: Option<String> },
}
pub struct FreezeCommand {
    pub input: FreezeInput,
    pub audit: AuditContext,
}
pub struct FreezeResult {
    pub status: u16,
    pub body: Value,
}
impl MarketCommand for FreezeCommand {
    type Output = FreezeResult;
    fn prepare_response(&self, value: &Self::Output) -> Result<(), MarketAttemptError> {
        serde_json::to_vec(&value.body)
            .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?;
        Ok(())
    }
    fn preview<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            // Resource lookup follows locked account/role checks and validation.
            let parties = PartyLockPreview::new([], [], actor, json!({"actor":actor}));
            let mut locks = OperationLockPlan::new(MarketOperation::Freeze);
            locks.parties(&parties)?;
            Ok(MarketPreview { parties, locks })
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            require_admin(actor)?;
            match &self.input {
                FreezeInput::List { query } => {
                    let page = FrozenPage::parse(query.as_deref())?;
                    let body = tx.freeze(actor)?.list(&page).await?;
                    Ok(FreezeResult { status: 200, body })
                }
                FreezeInput::Freeze { trade, body } => {
                    let value: Value = serde_json::from_slice(body).map_err(|_| validation())?;
                    let request = FreezeRequest::parse(&value)?;
                    let trade = trade.parse().map_err(|_| validation())?;
                    let (row, audit) = tx.freeze(actor)?.freeze(trade, &request).await?;
                    let status = if audit.is_some() { 201 } else { 200 };
                    if let Some(audit) = audit {
                        tx.freeze_audit(audit, &self.audit).await?;
                    }
                    Ok(FreezeResult {
                        status,
                        body: serde_json::to_value(row)
                            .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?,
                    })
                }
                FreezeInput::Unfreeze { trade, body } => {
                    let value: Value = serde_json::from_slice(body).map_err(|_| validation())?;
                    let request = UnfreezeRequest::parse(&value)?;
                    let trade = trade.parse().map_err(|_| validation())?;
                    let (row, audit) = tx.freeze(actor)?.unfreeze(trade, &request).await?;
                    tx.freeze_audit(audit, &self.audit).await?;
                    Ok(FreezeResult {
                        status: 200,
                        body: serde_json::to_value(row)
                            .map_err(|_| MarketWriteError::new(MarketErrorCode::DatabaseError))?,
                    })
                }
            }
        })
    }
}
fn require_admin(actor: &AuthenticatedActor) -> Result<(), MarketWriteError> {
    if actor.account().role != ActorRole::Admin {
        return Err(MarketWriteError::new(MarketErrorCode::SupportForbidden));
    }
    Ok(())
}

pub(super) struct FreezeAudit {
    pub actor: Uuid,
    pub trade: Uuid,
    pub cleared: bool,
    pub reason: FreezeReason,
    pub note: String,
    pub actor_is_party: bool,
    pub at: DateTime<Utc>,
}
impl FreezeAudit {
    pub(super) fn changes(&self) -> Value {
        json!({"status":{"before":if self.cleared {Some("ACTIVE")} else {None},"after":if self.cleared {"CLEARED"} else {"ACTIVE"}},"reason_code":self.reason,"note":self.note,"actor":self.actor,"actor_is_party":self.actor_is_party})
    }
}

pub(super) struct FreezePort<'a> {
    conn: &'a mut PgConnection,
    actor: &'a AuthenticatedActor,
}
impl<'a> FreezePort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &Locked,
        actor: &'a AuthenticatedActor,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        require_admin(actor)?;
        if !locked.contains(LockClass::User, actor.user_id()) {
            return Err(MarketWriteError::new(MarketErrorCode::MarketSliceChanged));
        }
        Ok(Self { conn, actor })
    }
    async fn trade(&mut self, trade: Uuid) -> Result<(String, bool), MarketWriteError> {
        // F1 and F2 share the lifecycle's trade row, held until the owner commits.
        let row = sqlx::query("SELECT status::text AS status,buyer_id,seller_id,buyer_user_id,seller_user_id FROM trades WHERE id=$1 FOR UPDATE")
            .bind(trade).fetch_optional(&mut *self.conn).await?.ok_or_else(|| MarketWriteError::new(MarketErrorCode::TradeNotFound))?;
        let user = self.actor.user_id();
        let organization = self.actor.account().current_organization_id;
        let party = [
            row.try_get::<Option<Uuid>, _>("buyer_user_id")?,
            row.try_get("seller_user_id")?,
        ]
        .contains(&Some(user))
            || organization.is_some_and(|id| {
                [row.get::<Uuid, _>("buyer_id"), row.get("seller_id")].contains(&id)
            });
        Ok((row.try_get("status")?, party))
    }
    async fn active(&mut self, trade: Uuid) -> Result<Option<FreezeResponse>, MarketWriteError> {
        let row = sqlx::query("SELECT id,trade_id,status,reason_code,note,frozen_by_user_id,frozen_at,cleared_by_user_id,cleared_at,clear_note,version FROM trade_freezes WHERE trade_id=$1 AND status='ACTIVE' FOR UPDATE")
            .bind(trade).fetch_optional(&mut *self.conn).await?;
        row.as_ref().map(project).transpose()
    }
    async fn freeze(
        &mut self,
        trade: Uuid,
        request: &FreezeRequest,
    ) -> Result<(FreezeResponse, Option<FreezeAudit>), MarketWriteError> {
        let (status, party) = self.trade(trade).await?;
        if let Some(row) = self.active(trade).await? {
            if row.frozen_by_user_id == self.actor.user_id()
                && row.reason_code == request.reason_code
            {
                return Ok((row, None));
            }
            return Err(MarketWriteError::new(MarketErrorCode::TradeAlreadyFrozen));
        }
        if !matches!(
            status.as_str(),
            "PENDING_CONFIRMATION" | "CONFIRMED" | "DELIVERED"
        ) {
            return Err(MarketWriteError::new(MarketErrorCode::TradeNotSettleable));
        }
        let at = Utc::now();
        let row = sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id,frozen_at,version) VALUES($1,$2,'ACTIVE',$3,$4,$5,$6,1) RETURNING id,trade_id,status,reason_code,note,frozen_by_user_id,frozen_at,cleared_by_user_id,cleared_at,clear_note,version")
            .bind(Uuid::new_v4()).bind(trade).bind(request.reason_code.text()).bind(&request.note).bind(self.actor.user_id()).bind(at).fetch_one(&mut *self.conn).await?;
        Ok((
            project(&row)?,
            Some(FreezeAudit {
                actor: self.actor.user_id(),
                trade,
                cleared: false,
                reason: request.reason_code,
                note: request.note.clone(),
                actor_is_party: party,
                at,
            }),
        ))
    }
    async fn unfreeze(
        &mut self,
        trade: Uuid,
        request: &UnfreezeRequest,
    ) -> Result<(FreezeResponse, FreezeAudit), MarketWriteError> {
        let (_, party) = self.trade(trade).await?;
        let previous = self
            .active(trade)
            .await?
            .ok_or_else(|| MarketWriteError::new(MarketErrorCode::TradeNotFrozen))?;
        let at = Utc::now();
        let row = sqlx::query("UPDATE trade_freezes SET status='CLEARED',cleared_by_user_id=$2,cleared_at=$3,clear_note=$4,version=version+1 WHERE id=$1 RETURNING id,trade_id,status,reason_code,note,frozen_by_user_id,frozen_at,cleared_by_user_id,cleared_at,clear_note,version")
            .bind(previous.id).bind(self.actor.user_id()).bind(at).bind(&request.note).fetch_one(&mut *self.conn).await?;
        Ok((
            project(&row)?,
            FreezeAudit {
                actor: self.actor.user_id(),
                trade,
                cleared: true,
                reason: previous.reason_code,
                note: request.note.clone(),
                actor_is_party: party,
                at,
            },
        ))
    }
    async fn list(&mut self, page: &FrozenPage) -> Result<Value, MarketWriteError> {
        // Count and page share one statement snapshot of the ACTIVE relation.
        let row = sqlx::query("WITH active AS MATERIALIZED (SELECT f.id,f.trade_id,t.status::text AS trade_status,f.reason_code,f.note,f.frozen_at,f.frozen_by_user_id FROM trade_freezes f JOIN trades t ON t.id=f.trade_id WHERE f.status='ACTIVE'), page AS (SELECT * FROM active ORDER BY frozen_at DESC,id DESC OFFSET $1 LIMIT $2) SELECT (SELECT count(*) FROM active) AS total,COALESCE((SELECT jsonb_agg(jsonb_build_object('id',id,'trade_id',trade_id,'trade_status',trade_status,'reason_code',reason_code,'note',note,'frozen_at',frozen_at,'frozen_by_user_id',frozen_by_user_id) ORDER BY frozen_at DESC,id DESC) FROM page),'[]'::jsonb) AS items")
            .bind(page.skip).bind(page.limit).fetch_one(&mut *self.conn).await?;
        Ok(
            json!({"items":row.try_get::<Value,_>("items")?,"total":row.try_get::<i64,_>("total")?,"skip":page.skip,"limit":page.limit}),
        )
    }
}

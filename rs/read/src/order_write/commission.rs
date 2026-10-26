//! Administrator views over the stored legacy commission rows.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::authority::ActorRole;
use super::effects::{MutationEffect, MutationKind};
use super::locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview};
use super::transaction::{
    BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
};
use super::{AuthenticatedActor, MarketErrorCode, MarketWriteError};
use crate::auth::limits::SharedLimits;
use crate::{Config, MarketPolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommissionStatus {
    Pending,
    Invoiced,
    Paid,
}
impl CommissionStatus {
    fn text(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Invoiced => "INVOICED",
            Self::Paid => "PAID",
        }
    }
    fn parse(value: &str) -> Result<Self, MarketWriteError> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "INVOICED" => Ok(Self::Invoiced),
            "PAID" => Ok(Self::Paid),
            _ => Err(validation()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchField<T> {
    Keep,
    Set(Option<T>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommissionPatch {
    pub status: Option<CommissionStatus>,
    pub invoice_number: PatchField<String>,
    pub invoice_date: PatchField<NaiveDate>,
    pub payment_date: PatchField<NaiveDate>,
    pub notes: PatchField<String>,
}
impl CommissionPatch {
    pub fn parse(value: Value) -> Result<Self, MarketWriteError> {
        let object = value.as_object().ok_or_else(validation)?;
        fn text(
            object: &serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<PatchField<String>, MarketWriteError> {
            match object.get(name) {
                None => Ok(PatchField::Keep),
                Some(Value::Null) => Ok(PatchField::Set(None)),
                Some(Value::String(s)) => Ok(PatchField::Set(Some(s.clone()))),
                _ => Err(validation()),
            }
        }
        fn date(
            object: &serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<PatchField<NaiveDate>, MarketWriteError> {
            match text(object, name)? {
                PatchField::Keep => Ok(PatchField::Keep),
                PatchField::Set(None) => Ok(PatchField::Set(None)),
                PatchField::Set(Some(s)) => Ok(PatchField::Set(Some(
                    NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|_| validation())?,
                ))),
            }
        }
        Ok(Self {
            status: object
                .get("status")
                .map(|s| CommissionStatus::parse(s.as_str().ok_or_else(validation)?))
                .transpose()?,
            invoice_number: text(object, "invoice_number")?,
            invoice_date: date(object, "invoice_date")?,
            payment_date: date(object, "payment_date")?,
            notes: text(object, "notes")?,
        })
    }
}
fn validation() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}

pub struct CommissionResult {
    pub status: u16,
    pub body: Value,
}
impl CommissionResult {
    fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }
    fn refusal(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            body: json!({"detail":{"code":code,"message":message}}),
        }
    }
}
#[derive(Clone)]
pub enum CommissionOperation {
    List,
    Summary,
    Patch { id: Uuid, body: Value },
}
pub struct CommissionCommand {
    pub operation: CommissionOperation,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
    pub limits: SharedLimits,
    pub client: String,
    pub at: DateTime<Utc>,
    pub audit_context: AuditContext,
}
impl MarketCommand for CommissionCommand {
    type Output = CommissionResult;
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
            let parties = PartyLockPreview::new([], [], actor, json!({"actor":actor}));
            let mut locks = OperationLockPlan::new(MarketOperation::Commission);
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
            if actor.account().role != ActorRole::Admin {
                return Ok(CommissionResult::refusal(
                    403,
                    "ADMIN_REQUIRED",
                    "Administrator access is required",
                ));
            }
            let path = match self.operation {
                CommissionOperation::List => "/api/orders/admin/commissions",
                CommissionOperation::Summary => "/api/orders/admin/commissions/summary",
                CommissionOperation::Patch { .. } => {
                    "/api/orders/admin/commissions/{commission_id}"
                }
            };
            let patch = match &self.operation {
                CommissionOperation::Patch { body, .. } => {
                    match CommissionPatch::parse(body.clone()) {
                        Ok(p) => Some(p),
                        Err(_) => {
                            return Ok(CommissionResult::refusal(
                                422,
                                "VALIDATION_FAILED",
                                "The commission patch is not valid",
                            ));
                        }
                    }
                }
                _ => None,
            };
            if let Err(error) =
                self.limits
                    .check_endpoint(&self.client, path, self.at.timestamp().max(0) as u64)
            {
                return Ok(CommissionResult {
                    status: error.status.as_u16(),
                    body: json!({"detail":error.detail}),
                });
            }
            match &self.operation {
                CommissionOperation::List => {
                    Ok(CommissionResult::ok(tx.commissions(actor)?.list().await?))
                }
                CommissionOperation::Summary => Ok(CommissionResult::ok(
                    tx.commissions(actor)?.summary().await?,
                )),
                CommissionOperation::Patch { id, .. } => {
                    let before = tx
                        .effects()?
                        .capture_before(&self.policy, &self.config, &[], self.at)
                        .await?;
                    let Some((response, changes)) = tx
                        .commissions(actor)?
                        .patch(*id, patch.as_ref().expect("validated patch"))
                        .await?
                    else {
                        return Ok(CommissionResult::refusal(
                            404,
                            "COMMISSION_NOT_FOUND",
                            "The commission was not found",
                        ));
                    };
                    let mut set = before.stage();
                    set.push(MutationEffect {
                        kind: MutationKind::CommissionChanged,
                        actor_id: actor.user_id(),
                        organization_id: actor
                            .account()
                            .current_organization_id
                            .unwrap_or(Uuid::nil()),
                        resource_type: "commission".into(),
                        resource_id: *id,
                        changes,
                        event: None,
                        at: self.at,
                        audit_context: self.audit_context.clone(),
                    })?;
                    tx.effects()?
                        .finish(&self.policy, &self.config, set)
                        .await?;
                    Ok(CommissionResult::ok(response))
                }
            }
        })
    }
}

pub(super) struct CommissionPort<'a> {
    conn: &'a mut PgConnection,
}
impl<'a> CommissionPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &Locked,
        actor: &AuthenticatedActor,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        if actor.account().role != ActorRole::Admin
            || !locked.contains(LockClass::User, actor.user_id())
        {
            return Err(MarketWriteError::new(
                MarketErrorCode::ExecutionNotAvailable,
            ));
        }
        Ok(Self { conn })
    }
    async fn list(&mut self) -> Result<Value, MarketAttemptError> {
        let rows=sqlx::query("SELECT id,match_id,amount_usd::text AS amount_usd,status,invoice_number,invoice_date,payment_date,created_at FROM commissions ORDER BY created_at DESC,id DESC").fetch_all(&mut *self.conn).await?;
        Ok(Value::Array(
            rows.iter().map(project).collect::<Result<_, _>>()?,
        ))
    }
    async fn summary(&mut self) -> Result<Value, MarketAttemptError> {
        let rows=sqlx::query("SELECT status,count(*) AS count,coalesce(sum(amount_usd),0)::text AS total FROM commissions GROUP BY status").fetch_all(&mut *self.conn).await?;
        let mut body = json!({"pending_count":0,"invoiced_count":0,"paid_count":0,"total_pending_usd":"0","total_invoiced_usd":"0","total_paid_usd":"0"});
        for row in rows {
            let status = CommissionStatus::parse(row.try_get::<&str, _>("status")?)?;
            let name = status.text().to_ascii_lowercase();
            let raw: String = row.try_get("total")?;
            let amount = Decimal::from_str_exact(&raw)
                .map_err(|_| MarketWriteError::new(MarketErrorCode::ValueOverflow))?;
            body[format!("{name}_count")] = json!(row.try_get::<i64, _>("count")?);
            body[format!("total_{name}_usd")] = json!(amount.to_string());
        }
        Ok(body)
    }
    async fn patch(
        &mut self,
        id: Uuid,
        patch: &CommissionPatch,
    ) -> Result<Option<(Value, Value)>, MarketAttemptError> {
        let row=sqlx::query("SELECT id,match_id,amount_usd::text AS amount_usd,status,invoice_number,invoice_date,payment_date,created_at,notes FROM commissions WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *self.conn).await?;
        let Some(row) = row else { return Ok(None) };
        let previous: String = row.try_get("status")?;
        let status = patch
            .status
            .map(CommissionStatus::text)
            .unwrap_or(&previous);
        fn apply<T: Clone>(field: &PatchField<T>, previous: Option<T>) -> Option<T> {
            match field {
                PatchField::Keep => previous,
                PatchField::Set(value) => value.clone(),
            }
        }
        let invoice = apply(&patch.invoice_number, row.try_get("invoice_number")?);
        let invoice_date = apply(&patch.invoice_date, row.try_get("invoice_date")?);
        let payment_date = apply(&patch.payment_date, row.try_get("payment_date")?);
        let notes = apply(&patch.notes, row.try_get("notes")?);
        sqlx::query("UPDATE commissions SET status=$2,invoice_number=$3,invoice_date=$4,payment_date=$5,notes=$6,updated_at=$7 WHERE id=$1").bind(id).bind(status).bind(&invoice).bind(invoice_date).bind(payment_date).bind(notes).bind(Utc::now()).execute(&mut *self.conn).await?;
        let mut response = project(&row)?;
        response["status"] = json!(status);
        response["invoice_number"] = json!(invoice);
        response["invoice_date"] = json!(invoice_date);
        response["payment_date"] = json!(payment_date);
        let changes = if status != previous {
            json!({"status":{"from":previous,"to":status}})
        } else {
            json!({})
        };
        Ok(Some((response, changes)))
    }
}
fn project(row: &sqlx::postgres::PgRow) -> Result<Value, sqlx::Error> {
    Ok(
        json!({"id":row.try_get::<Uuid,_>("id")?,"match_id":row.try_get::<Uuid,_>("match_id")?,"amount_usd":row.try_get::<String,_>("amount_usd")?,"status":row.try_get::<String,_>("status")?,"invoice_number":row.try_get::<Option<String>,_>("invoice_number")?,"invoice_date":row.try_get::<Option<NaiveDate>,_>("invoice_date")?,"payment_date":row.try_get::<Option<NaiveDate>,_>("payment_date")?,"created_at":row.try_get::<DateTime<Utc>,_>("created_at")?}),
    )
}

pub use super::effects::AuditContext;
pub async fn run_commission_command(
    pool: sqlx::PgPool,
    permits: Arc<tokio::sync::Semaphore>,
    access: crate::auth::VerifiedAccess,
    command: CommissionCommand,
) -> Result<CommissionResult, MarketWriteError> {
    super::transaction::run_market_command(
        pool,
        permits,
        access,
        command,
        super::AdmissionClass::Execution,
    )
    .await
}

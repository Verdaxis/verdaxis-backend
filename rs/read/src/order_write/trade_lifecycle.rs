//! Confirmation and recorded-party decline inside the sealed market owner.
use super::{
    AdmissionClass, AuthenticatedActor, Canonical, CommittedCommand, MarketErrorCode,
    MarketWriteError, Operation, ReceiptRecord, StockQuantity,
    authority::ActorRole,
    effects::AuditContext,
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview, SliceKey},
    matching::{self, CreationMethod, MatchBound, OrderRow, OwnerAdmission, Side},
    transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
        run_market_command,
    },
};
use crate::{AppState, Config, MarketPolicy};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleAction {
    Confirm,
    Decline,
}
impl LifecycleAction {
    fn operation(self) -> Operation {
        match self {
            Self::Confirm => Operation::TradeConfirm,
            Self::Decline => Operation::TradeDecline,
        }
    }
    pub fn status(self) -> &'static str {
        match self {
            Self::Confirm => "CONFIRMED",
            Self::Decline => "DECLINED",
        }
    }
    fn admission(self) -> AdmissionClass {
        if self == Self::Confirm {
            AdmissionClass::Execution
        } else {
            AdmissionClass::Cleanup
        }
    }
}
pub fn lifecycle_hash(action: LifecycleAction, trade: Uuid) -> Result<String, MarketWriteError> {
    let value = json!({"operation":action.operation().as_str(),"trade_id":trade});
    Ok(super::command_request_hash(
        &Canonical::from_json(&value).map_err(|_| fault())?,
    ))
}
fn fault() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn invalid_parties() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::TradePartiesInvalid)
}
fn forbidden() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::TradePartyForbidden)
}
fn status_error(code: MarketErrorCode, status: StatusCode) -> MarketWriteError {
    let mut e = MarketWriteError::new(code);
    e.status = status;
    e
}
fn uuid(row: &Value, key: &str) -> Option<Uuid> {
    row[key].as_str().and_then(|s| s.parse().ok())
}
fn required_uuid(row: &Value, key: &str) -> Result<Uuid, MarketWriteError> {
    uuid(row, key).ok_or_else(invalid_parties)
}

#[derive(Clone)]
pub(crate) struct LifecycleRequest {
    pub id: Uuid,
    pub action: LifecycleAction,
    pub key: Option<String>,
    pub audit: AuditContext,
}
struct LifecycleCommand {
    request: LifecycleRequest,
    access: Arc<crate::auth::VerifiedAccess>,
    policy: Arc<MarketPolicy>,
    config: Arc<Config>,
    hash: String,
}
async fn load_trade(conn: &mut PgConnection, id: Uuid) -> Result<Option<Value>, MarketWriteError> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(t)||jsonb_build_object('quantity_mt',t.quantity_mt::text,'price_per_mt_usd',t.price_per_mt_usd::text,'commission_rate_pct',t.commission_rate_pct::text,'commission_fee_per_mt_usd',t.commission_fee_per_mt_usd::text,'commission_amount_usd',t.commission_amount_usd::text,'final_quantity_mt',t.final_quantity_mt::text,'final_price_per_mt',t.final_price_per_mt::text,'final_total_usd',t.final_total_usd::text,'buyer_name',COALESCE(b.name,''),'seller_name',COALESCE(s.name,''),'region',COALESCE(t.delivery_point_region,'')) FROM trades t LEFT JOIN organizations b ON b.id=t.buyer_id LEFT JOIN organizations s ON s.id=t.seller_id WHERE t.id=$1").bind(id).fetch_optional(conn).await?)
}
async fn linked_orders(
    conn: &mut PgConnection,
    trade: &Value,
) -> Result<Vec<OrderRow>, MarketWriteError> {
    let mut rows = Vec::new();
    for key in ["ask_order_id", "bid_order_id"] {
        if let Some(id) = uuid(trade, key) {
            rows.push(matching::load_order(conn, id).await.map_err(|e| {
                if e.code == MarketErrorCode::DatabaseError {
                    invalid_parties()
                } else {
                    e
                }
            })?);
        }
    }
    Ok(rows)
}
fn trade_slice(trade: &Value, orders: &[OrderRow]) -> Option<SliceKey> {
    if let (Some(product), Some(window)) = (
        uuid(trade, "product_id"),
        trade["availability_window"]
            .as_str()
            .filter(|s| !s.is_empty()),
    ) {
        Some(SliceKey {
            product_id: product,
            delivery_point_id: uuid(trade, "delivery_point_id"),
            availability_window: window.into(),
        })
    } else {
        orders.first().map(OrderRow::slice)
    }
}
impl LifecycleCommand {
    async fn discover(
        &self,
        conn: &mut PgConnection,
        actor: Uuid,
    ) -> Result<MarketPreview, MarketAttemptError> {
        let authenticated = super::authority::admission::authenticate_preview(
            conn,
            self.access.clone(),
            self.request.action.admission(),
        )
        .await?;
        let org = authenticated.account().current_organization_id;
        if self.request.key.is_some() && org.is_none() {
            return Err(forbidden().into());
        }
        let replay = if let Some(key) = &self.request.key {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM market_command_results WHERE actor_user_id=$1 AND operation=$2 AND idempotency_key=$3)").bind(actor).bind(self.request.action.operation().as_str()).bind(key).fetch_one(&mut *conn).await?
        } else {
            false
        };
        let mut organizations: Vec<_> = org.into_iter().collect();
        let mut users = Vec::new();
        let mut facts = json!({"organization":org,"replay":replay});
        let mut orders = Vec::new();
        let mut slice = None;
        if !replay && let Some(trade) = load_trade(conn, self.request.id).await? {
            organizations.extend([
                required_uuid(&trade, "buyer_id")?,
                required_uuid(&trade, "seller_id")?,
            ]);
            users.extend(
                [
                    uuid(&trade, "buyer_user_id"),
                    uuid(&trade, "seller_user_id"),
                ]
                .into_iter()
                .flatten(),
            );
            orders = linked_orders(conn, &trade).await?;
            for row in &orders {
                organizations.push(row.organization_id);
                users.extend(row.owner_user_id);
                if row.creation_method == CreationMethod::MarketSupport {
                    let members:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 AND role::text IN ('BUYER','SUPPLIER') ORDER BY id").bind(row.organization_id).fetch_all(&mut *conn).await?;
                    users.extend(members);
                }
            }
            slice = trade_slice(&trade, &orders);
            facts["trade"] = json!({"buyer":trade["buyer_id"],"seller":trade["seller_id"],"buyer_user":trade["buyer_user_id"],"seller_user":trade["seller_user_id"],"ask":trade["ask_order_id"],"bid":trade["bid_order_id"],"product":trade["product_id"],"point":trade["delivery_point_id"],"window":trade["availability_window"]});
            facts["orders"]=json!(orders.iter().map(|o|json!({"id":o.id,"org":o.organization_id,"owner":o.owner_user_id,"creator":o.created_by_actor_user_id,"method":format!("{:?}",o.creation_method),"item":o.inventory_item_id,"product":o.product_id,"point":o.delivery_point_id,"window":o.availability_window})).collect::<Vec<_>>());
        }
        if let Some(item) = orders.first().and_then(|o| o.inventory_item_id) {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inventory_items WHERE id=$1)")
                    .bind(item)
                    .fetch_one(&mut *conn)
                    .await?;
            if !exists {
                return Err(MarketWriteError::new(MarketErrorCode::InventoryInconsistent).into());
            }
        }
        let has_trade = facts.get("trade").is_some();
        let parties = PartyLockPreview::new(organizations, users, actor, facts);
        let mut locks =
            OperationLockPlan::new(if self.request.action == LifecycleAction::Confirm {
                MarketOperation::Confirm
            } else {
                MarketOperation::Decline
            });
        locks.parties(&parties)?;
        if let Some(key) = &self.request.key {
            locks.idempotency(actor, self.request.action.operation().as_str(), key)?;
        }
        if let Some(slice) = slice {
            locks.slices([slice])?;
        }
        if !replay && has_trade {
            locks.trade(self.request.id)?;
        }
        locks.orders(orders.iter().map(|o| o.id))?;
        locks.inventory(orders.first().and_then(|o| o.inventory_item_id))?;
        Ok(MarketPreview { parties, locks })
    }
}
impl MarketCommand for LifecycleCommand {
    type Output = CommittedCommand;
    fn replans_missing_lock_rows(&self) -> bool {
        true
    }
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(self.discover(conn, actor))
    }
    fn replay<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
        Box::pin(async move {
            let Some(key) = &self.request.key else {
                return Ok(None);
            };
            let context = tx
                .lifecycle_context(
                    actor,
                    actor
                        .account()
                        .current_organization_id
                        .ok_or_else(forbidden)?,
                    Utc::now(),
                )
                .await?;
            Ok(tx
                .replay_receipt(
                    &context,
                    self.request.action.operation(),
                    self.request.id,
                    key,
                    &self.hash,
                )
                .await?)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            // The owner has acquired and revalidated all row locks before this clock.
            let now = Utc::now();
            if self.request.action == LifecycleAction::Confirm {
                tx.lifecycle()?
                    .actor_execution(actor, &self.config, now)
                    .await?;
            }
            let outcome = tx
                .lifecycle()?
                .transition(actor, &self.request, &self.policy, &self.config, now)
                .await?;
            let body = super::trades::project_trade(
                outcome.trade.clone(),
                Some(outcome.viewer),
                actor.account().role == ActorRole::Admin,
            )?;
            let context = if self.request.key.is_some() {
                Some(
                    tx.lifecycle_context(
                        actor,
                        actor
                            .account()
                            .current_organization_id
                            .ok_or_else(forbidden)?,
                        now,
                    )
                    .await?,
                )
            } else {
                None
            };
            tx.effects()?
                .trade_lifecycle(
                    &self.policy,
                    &self.config,
                    outcome,
                    &self.request,
                    actor.user_id(),
                    now,
                )
                .await?;
            if let (Some(key), Some(context)) = (&self.request.key, context) {
                tx.record_receipt(
                    &context,
                    &ReceiptRecord {
                        id: Uuid::new_v4(),
                        actor_user_id: actor.user_id(),
                        effective_organization_id: context.effective_organization_id(),
                        support_context_id: None,
                        operation: self.request.action.operation(),
                        idempotency_key: key.clone(),
                        request_hash: self.hash.clone(),
                        resource_id: self.request.id,
                        response_status: 200,
                        response_body: body.clone(),
                    },
                )
                .await?;
            }
            Ok(CommittedCommand::new(200, body, false))
        })
    }
}
pub(crate) async fn authenticate_request(
    app: &AppState,
    access: &crate::auth::VerifiedAccess,
    action: LifecycleAction,
) -> Result<(), MarketWriteError> {
    let shared = Arc::new(crate::auth::VerifiedAccess {
        subject: access.subject,
        issued_at: access.issued_at,
        issuance_out_of_range: access.issuance_out_of_range,
    });
    let mut conn = app.pools.read.acquire().await?;
    super::authority::admission::authenticate_preview(&mut conn, shared, action.admission())
        .await?;
    Ok(())
}

pub(crate) async fn dispatch(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: LifecycleRequest,
) -> Result<CommittedCommand, MarketWriteError> {
    let class = request.action.admission();
    let hash = lifecycle_hash(request.action, request.id)?;
    let shared = Arc::new(crate::auth::VerifiedAccess {
        subject: access.subject,
        issued_at: access.issued_at,
        issuance_out_of_range: access.issuance_out_of_range,
    });
    run_market_command(
        app.pools.read,
        app.reads,
        access,
        LifecycleCommand {
            request,
            access: shared,
            policy: app.policy,
            config: app.config,
            hash,
        },
        class,
    )
    .await
}

pub(super) struct LifecycleOutcome {
    pub trade: Value,
    pub viewer: Uuid,
    pub initiator: Uuid,
    pub before: super::effects::BeforeEffects,
    pub changed_order: Option<Uuid>,
    pub expired_remainder: Option<Decimal>,
    pub public_invalidated: bool,
}
pub(super) struct LifecyclePort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
}
impl<'a> LifecyclePort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &'a Locked,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self { conn, locked })
    }
    async fn actor_execution(
        &mut self,
        actor: &AuthenticatedActor,
        config: &Config,
        now: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        if actor.account().role == ActorRole::Admin {
            return Err(forbidden());
        }
        let row=sqlx::query("SELECT u.kyc_status,u.kyc_organization_id,g.expires_at FROM users u LEFT JOIN kyc_execution_grace g ON g.user_id=u.id AND g.organization_id=u.organization_id WHERE u.id=$1").bind(actor.user_id()).fetch_one(&mut *self.conn).await?;
        let status: String = row.get("kyc_status");
        let grace = actor
            .account()
            .current_organization_id
            .and_then(|organization_id| {
                row.get::<Option<DateTime<Utc>>, _>("expires_at")
                    .map(|expires_at| crate::kyc::grace::Grace {
                        user_id: actor.user_id(),
                        organization_id,
                        expires_at,
                    })
            });
        let result = crate::kyc::policy::eligibility(
            &crate::kyc::policy::KycPolicy {
                execution_gate_enabled: config.kyc_execution_gate_enabled,
            },
            &crate::kyc::policy::KycParty {
                user_id: actor.user_id(),
                organization_id: actor.account().current_organization_id,
                kyc_organization_id: row.get("kyc_organization_id"),
                kyc_status: serde_json::from_value(json!(status)).map_err(|_| fault())?,
            },
            grace.as_ref(),
            now,
        );
        if !result.eligible {
            return Err(MarketWriteError::kyc_required(
                status,
                result.grace_expires_at,
            ));
        }
        let org = actor
            .account()
            .current_organization_id
            .ok_or_else(forbidden)?;
        super::authority::admission::require_execution(
            self.conn,
            self.locked,
            &super::PartyRef {
                user_id: actor.user_id(),
                organization_id: org,
                role: actor.account().role,
            },
            now,
            config.kyc_execution_gate_enabled,
        )
        .await
    }
    async fn parties(
        &mut self,
        trade: &Value,
        orders: &[OrderRow],
        bound: MatchBound<'_>,
        gate_enabled: bool,
    ) -> Result<(), MarketWriteError> {
        for (side, user_key, org_key, order_key) in [
            (Side::Bid, "buyer_user_id", "buyer_id", "bid_order_id"),
            (Side::Ask, "seller_user_id", "seller_id", "ask_order_id"),
        ] {
            let user = required_uuid(trade, user_key)?;
            let organization = required_uuid(trade, org_key)?;
            if !self.locked.contains(LockClass::User, user)
                || !self.locked.contains(LockClass::Organization, organization)
            {
                return Err(invalid_parties());
            }
            let admitted = if let Some(order) =
                orders.iter().find(|o| Some(o.id) == uuid(trade, order_key))
            {
                if order.organization_id != organization
                    || order.owner_user_id != Some(user)
                    || order.side != side
                {
                    false
                } else {
                    matching::OwnerRevalidation::default()
                        .revalidate(self.conn, self.locked, order, &bound)
                        .await?
                        == OwnerAdmission::Admitted
                }
            } else {
                let role: Option<String> =
                    sqlx::query_scalar("SELECT role::text FROM users WHERE id=$1")
                        .bind(user)
                        .fetch_optional(&mut *self.conn)
                        .await?;
                let role = match role.as_deref() {
                    Some("BUYER") => ActorRole::Buyer,
                    Some("SUPPLIER") => ActorRole::Supplier,
                    _ => return Err(invalid_parties()),
                };
                match super::authority::admission::require_execution(
                    self.conn,
                    self.locked,
                    &super::PartyRef {
                        user_id: user,
                        organization_id: organization,
                        role,
                    },
                    bound.now(),
                    gate_enabled,
                )
                .await
                {
                    Ok(()) => true,
                    Err(e)
                        if matches!(
                            e.code,
                            MarketErrorCode::KycRequired | MarketErrorCode::ExecutionNotAvailable
                        ) =>
                    {
                        false
                    }
                    Err(e) => return Err(e),
                }
            };
            if !admitted {
                return Err(invalid_parties());
            }
        }
        Ok(())
    }
    async fn authorize(
        &mut self,
        actor: &AuthenticatedActor,
        trade: &Value,
        order: Option<&OrderRow>,
        confirm: bool,
    ) -> Result<Uuid, MarketWriteError> {
        let seller = trade["initiated_by"] == "BUYER";
        let org = required_uuid(trade, if seller { "seller_id" } else { "buyer_id" })?;
        let recorded = uuid(
            trade,
            if seller {
                "seller_user_id"
            } else {
                "buyer_user_id"
            },
        );
        if let Some(order) = order.filter(|o| o.creation_method == CreationMethod::MarketSupport) {
            let (org_key, user_key, link_key, role) = if order.side == Side::Ask {
                (
                    "seller_id",
                    "seller_user_id",
                    "ask_order_id",
                    ActorRole::Supplier,
                )
            } else {
                (
                    "buyer_id",
                    "buyer_user_id",
                    "bid_order_id",
                    ActorRole::Buyer,
                )
            };
            if uuid(trade, org_key) != Some(order.organization_id)
                || uuid(trade, user_key) != order.owner_user_id
                || uuid(trade, link_key) != Some(order.id)
                || order.owner_user_id.is_none()
                || order.created_by_actor_user_id != order.owner_user_id
            {
                return Err(forbidden());
            }
            if Some(actor.user_id()) == order.owner_user_id {
                return Err(forbidden());
            }
            if actor.account().current_organization_id == Some(order.organization_id)
                && actor.account().role == role
            {
                if order.organization_id != org {
                    return Err(forbidden());
                }
                return Ok(org);
            }
        }
        let authorized = if confirm {
            recorded == Some(actor.user_id())
                && actor.account().current_organization_id == Some(org)
        } else {
            recorded.map_or(
                actor.account().current_organization_id == Some(org),
                |user| user == actor.user_id(),
            )
        };
        if !authorized {
            return Err(forbidden());
        }
        Ok(org)
    }
    async fn transition(
        &mut self,
        actor: &AuthenticatedActor,
        request: &LifecycleRequest,
        policy: &MarketPolicy,
        config: &Config,
        now: DateTime<Utc>,
    ) -> Result<LifecycleOutcome, MarketWriteError> {
        let mut trade = load_trade(self.conn, request.id)
            .await?
            .ok_or_else(|| MarketWriteError::new(MarketErrorCode::TradeNotFound))?;
        if !self.locked.contains(LockClass::Trade, request.id) {
            return Err(invalid_parties());
        }
        let orders = linked_orders(self.conn, &trade).await?;
        let order = orders.first();
        if let (Some(slice), Some(order)) = (trade_slice(&trade, &orders), order)
            && slice != order.slice()
        {
            return Err(MarketWriteError::new(MarketErrorCode::MarketSliceChanged));
        }
        let confirm = request.action == LifecycleAction::Confirm;
        if confirm {
            for key in ["buyer_id", "seller_id"] {
                let approved: bool = sqlx::query_scalar(
                    "SELECT verification_status::text='APPROVED' FROM organizations WHERE id=$1",
                )
                .bind(required_uuid(&trade, key)?)
                .fetch_one(&mut *self.conn)
                .await?;
                if !approved {
                    return Err(forbidden());
                }
            }
        }
        if trade["status"] != "PENDING_CONFIRMATION" {
            return Err(status_error(
                MarketErrorCode::TradeNotPending,
                StatusCode::BAD_REQUEST,
            ));
        }
        if confirm && !provenance_compatible(&trade) {
            return Err(invalid_parties());
        }
        let viewer = self.authorize(actor, &trade, order, confirm).await?;
        let bound = MatchBound::new(now)
            .with_policy(policy)
            .with_kyc_gate(config.kyc_execution_gate_enabled);
        if confirm {
            self.parties(&trade, &orders, bound, config.kyc_execution_gate_enabled)
                .await?;
            revalidate_terms(&trade, order, policy, now)?;
        }
        let quantity: Decimal = trade["quantity_mt"]
            .as_str()
            .ok_or_else(fault)?
            .parse()
            .map_err(|_| invalid_parties())?;
        let quantity = StockQuantity::checked(quantity)?;
        let changing = order.filter(|o| {
            !confirm
                && !matches!(
                    o.status,
                    matching::OrderStatus::Cancelled | matching::OrderStatus::Expired
                )
        });
        let ids: Vec<_> = changing.map(|o| o.id).into_iter().collect();
        let before = super::effects::EffectPort::new(self.conn, &ids, self.locked)
            .capture_before(policy, config, &ids, now)
            .await?;
        let public_before = if let Some(order) = order {
            super::effects::order_visible(self.conn, policy, config, order.id, now).await?
        } else {
            false
        };
        sqlx::query("UPDATE trades SET status=$2,confirmed_at=CASE WHEN $2='CONFIRMED' THEN $3 ELSE confirmed_at END WHERE id=$1").bind(request.id).bind(request.action.status()).bind(now).execute(&mut *self.conn).await?;
        let mut expired_remainder = None;
        if let Some(order) = order {
            let items: Vec<_> = order.inventory_item_id.into_iter().collect();
            let order_ids = [order.id];
            if confirm {
                if let Some(item) = order.inventory_item_id {
                    super::transaction::StockLedger::new(self.conn, &items, &order_ids)
                        .consume_reserved(item, quantity)
                        .await?;
                }
            } else if changing.is_some() {
                if order.expires_at.is_some_and(|at| at <= now) {
                    expired_remainder = Some(
                        order
                            .remaining_quantity_mt
                            .checked_add(quantity.value())
                            .ok_or_else(invalid_parties)?,
                    );
                }
                super::transaction::StockLedger::new(self.conn, &items, &order_ids)
                    .restore_order_remainder(order.id, quantity, now)
                    .await?;
            } else if let Some(item) = order.inventory_item_id {
                super::transaction::StockLedger::new(self.conn, &items, &order_ids)
                    .release(item, quantity, true)
                    .await?;
            }
        }
        if confirm {
            super::confirmed_effect::on_trade_confirmed(
                self.conn,
                request.id,
                required_uuid(&trade, "buyer_id")?,
                required_uuid(&trade, "seller_id")?,
                now,
            )
            .await?;
        }
        trade = load_trade(self.conn, request.id).await?.ok_or_else(fault)?;
        decorate_provenance(&mut trade);
        let public_invalidated = if confirm {
            let sql = crate::market::visibility::trade_query_sql(
                "SELECT id FROM eligible_trades WHERE id=$7",
            );
            crate::market::visibility::bind_trade_query(&sql, policy, &Default::default())?
                .bind(request.id)
                .fetch_optional(&mut *self.conn)
                .await?
                .is_some()
                || self.inventory_visible(order, policy).await?
        } else {
            public_before
                || if let Some(order) = order {
                    super::effects::order_visible(self.conn, policy, config, order.id, now).await?
                } else {
                    false
                }
        };
        let initiator = required_uuid(
            &trade,
            if trade["initiated_by"] == "BUYER" {
                "buyer_id"
            } else {
                "seller_id"
            },
        )?;
        Ok(LifecycleOutcome {
            trade,
            viewer,
            initiator,
            before,
            changed_order: changing.map(|o| o.id),
            expired_remainder,
            public_invalidated,
        })
    }
    async fn inventory_visible(
        &mut self,
        order: Option<&OrderRow>,
        policy: &MarketPolicy,
    ) -> Result<bool, MarketWriteError> {
        let Some(item) = order.and_then(|o| o.inventory_item_id) else {
            return Ok(false);
        };
        let row=sqlx::query("SELECT to_jsonb(i) AS item,p.id AS product_id,p.name AS product_name,p.fuel_type AS product_fuel,p.fuel_grade AS product_grade,p.is_active AS product_active,d.id AS point_id,d.name AS point_name,d.region AS point_region,d.is_active AS point_active,port.is_active AS port_active,o.provenance,o.verification_status::text AS org_status,u.status::text AS owner_status,u.kyc_status,u.organization_id AS owner_org FROM inventory_items i JOIN ports port ON port.id=i.port_id JOIN delivery_points d ON d.name=port.name JOIN products p ON p.name=i.product_name JOIN organizations o ON o.id=i.supplier_id JOIN users u ON u.id=i.owner_user_id WHERE i.id=$1").bind(item).fetch_optional(&mut *self.conn).await?;
        let Some(row) = row else { return Ok(false) };
        let item: Value = row.get("item");
        let product = policy.products.iter().find(|p| {
            p.id == row.get::<Uuid, _>("product_id")
                && p.name == row.get::<String, _>("product_name")
                && p.fuel_type == row.get::<String, _>("product_fuel")
                && p.fuel_grade == row.get::<String, _>("product_grade")
        });
        let Some(product) = product else {
            return Ok(false);
        };
        let point = row.get::<Uuid, _>("point_id");
        let canonical_point = policy.points.iter().any(|p| {
            p.id == point
                && p.name == row.get::<String, _>("point_name")
                && p.region == row.get::<String, _>("point_region")
        });
        let organization = uuid(&item, "supplier_id");
        let provenance: String = row.get("provenance");
        let evidence = provenance == "REAL"
            || (provenance == "DEMO"
                && organization.is_some_and(|org| {
                    crate::analytics::marketplace::demo_market_org_ids().contains(&org)
                }));
        if !canonical_point
            || product.execution_mode != "ORDERBOOK"
            || !product.lanes.contains(&point)
            || !row.get::<bool, _>("product_active")
            || !row.get::<bool, _>("point_active")
            || !row.get::<bool, _>("port_active")
            || row.get::<String, _>("org_status") != "APPROVED"
            || row.get::<String, _>("owner_status") != "APPROVED"
            || row.get::<String, _>("kyc_status") != "APPROVED"
            || row.get::<Option<Uuid>, _>("owner_org") != organization
            || !evidence
        {
            return Ok(false);
        }
        if let Some(standard) = &product.specification_standard {
            let present = |key: &str| item[key].as_str().is_some_and(|s| !s.trim().is_empty());
            return Ok(item["off_spec"] == false
                && item["specification_standard"]
                    .as_str()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case(standard))
                && present("carbon_intensity_method")
                && item["certification_declared"] == true
                && present("certification_scheme")
                && item["msds_available"] == true
                && !item["carbon_intensity_gco2_mj"].is_null()
                && present("feedstock")
                && present("origin"));
        }
        Ok(true)
    }
}
fn provenance_compatible(trade: &Value) -> bool {
    (trade["buyer_provenance"] == "REAL" && trade["seller_provenance"] == "REAL")
        || (trade["buyer_provenance"] == "DEMO"
            && trade["seller_provenance"] == "DEMO"
            && uuid(trade, "buyer_id") == Some(matching::DEMO_ACTIVITY_BUYER_ORG_ID)
            && uuid(trade, "seller_id") == Some(matching::DEMO_ACTIVITY_SELLER_ORG_ID))
}
fn decorate_provenance(trade: &mut Value) {
    let real = trade["buyer_provenance"] == "REAL" && trade["seller_provenance"] == "REAL";
    let demo = trade["buyer_provenance"] == "DEMO" && trade["seller_provenance"] == "DEMO";
    trade["source_kind"] = json!(if real {
        "CONFIRMED_TRADE"
    } else if demo {
        "DEMO_SEED"
    } else {
        "UNKNOWN"
    });
    trade["scope"] = json!(if uuid(trade, "delivery_point_id").is_some() {
        "DELIVERY_POINT"
    } else {
        "UNKNOWN"
    });
    trade["demo_status"] = json!(if real {
        "REAL_ONLY"
    } else if demo {
        "DEMO_ONLY"
    } else {
        "UNKNOWN"
    });
    trade["unknown_count"] = json!(u32::from(
        trade["buyer_provenance"] == "UNKNOWN" || trade["seller_provenance"] == "UNKNOWN"
    ));
}
fn revalidate_terms(
    trade: &Value,
    order: Option<&OrderRow>,
    policy: &MarketPolicy,
    now: DateTime<Utc>,
) -> Result<(), MarketWriteError> {
    let profile = policy
        .products
        .iter()
        .find(|p| Some(p.id) == uuid(trade, "product_id"))
        .map(|p| p.qualification_profile.as_str())
        .unwrap_or("scalar");
    for (field, expected) in [
        ("fame_terms_snapshot", "fame_v1"),
        ("lng_terms_snapshot", "lng_v1"),
    ] {
        let snapshot = &trade[field];
        if snapshot.is_null() {
            if profile == expected {
                return Err(if expected == "fame_v1" {
                    invalid_parties()
                } else {
                    MarketWriteError::new(MarketErrorCode::ValidationFailed)
                });
            }
            continue;
        }
        let terms_error = || {
            if expected == "fame_v1" {
                invalid_parties()
            } else {
                MarketWriteError::new(MarketErrorCode::ValidationFailed)
            }
        };
        if profile != expected {
            return Err(terms_error());
        }
        let today = now.with_timezone(&chrono_tz::Asia::Singapore).date_naive();
        if expected == "fame_v1" {
            crate::market::terms::validate_fame_terms("BID", Some(&snapshot["bid"]), today)
                .map_err(|_| terms_error())?;
            crate::market::terms::validate_fame_terms("ASK", Some(&snapshot["ask"]), today)
                .map_err(|_| terms_error())?;
        } else {
            crate::market::terms::validate_lng_terms("BID", Some(&snapshot["bid"]))
                .map_err(|_| terms_error())?;
            crate::market::terms::validate_lng_terms("ASK", Some(&snapshot["ask"]))
                .map_err(|_| terms_error())?;
        }
        let bid = crate::market::terms::owner_response_terms(expected, &snapshot["bid"])
            .map_err(|_| terms_error())?;
        let ask = crate::market::terms::owner_response_terms(expected, &snapshot["ask"])
            .map_err(|_| terms_error())?;
        let compatible = if expected == "fame_v1" {
            matching::fame_terms_compatible(
                &bid,
                &ask,
                now.with_timezone(&chrono_tz::Asia::Singapore).date_naive(),
            )
        } else {
            matching::lng_terms_compatible(&bid, &ask)
        };
        if !compatible {
            return Err(if expected == "lng_v1" {
                MarketWriteError::new(MarketErrorCode::LngTermsIncompatible)
            } else {
                invalid_parties()
            });
        }
        let order = order.ok_or_else(invalid_parties)?;
        if !matching::order_is_execution_qualified(order, MatchBound::new(now).with_policy(policy))
        {
            return Err(invalid_parties());
        }
        let current = if expected == "fame_v1" {
            order.terms.fame_terms.as_ref()
        } else {
            order.terms.lng_terms.as_ref()
        }
        .ok_or_else(terms_error)?;
        let current = crate::market::terms::owner_response_terms(expected, current)
            .map_err(|_| terms_error())?;
        if current != if order.side == Side::Ask { ask } else { bid } {
            return Err(invalid_parties());
        }
    }
    Ok(())
}

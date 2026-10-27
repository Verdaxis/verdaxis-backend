//! Pending direct takes and bounded participant trade projections.
use super::{
    AdmissionClass, AuthenticatedActor, Canonical, CommittedCommand, MarketErrorCode,
    MarketWriteError, TradeQuantity,
    authority::ActorRole,
    effects::AuditContext,
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview},
    matching::{self, MatchBound, OwnerAdmission, Side},
    transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
        run_market_command,
    },
};
use crate::{AppState, MarketPolicy};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use std::sync::Arc;
use uuid::Uuid;
fn invalid() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
fn fault() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn refusal(code: MarketErrorCode, status: StatusCode) -> MarketWriteError {
    let mut e = MarketWriteError::new(code);
    e.status = status;
    e
}
fn unavailable() -> MarketWriteError {
    refusal(MarketErrorCode::OrderUnavailable, StatusCode::BAD_REQUEST)
}

#[derive(Clone, Debug)]
pub struct TradeCreate {
    pub order_id: Uuid,
    pub quantity_mt: Decimal,
    pub expected_terms_digest: Option<String>,
    pub expected_order_version: Option<i64>,
    pub fame_terms: Option<Value>,
    pub lng_terms: Option<Value>,
    pub certification_declared: bool,
    pub msds_available: bool,
}
impl TradeCreate {
    pub fn parse(value: Value) -> Result<Self, MarketWriteError> {
        let object = value.as_object().ok_or_else(invalid)?;
        let order_id = object
            .get("order_id")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let quantity = object.get("quantity_mt").ok_or_else(invalid)?;
        let raw = quantity
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| quantity.to_string());
        let quantity_mt = TradeQuantity::parse_exact(&raw)?.value();
        let expected_terms_digest = object
            .get("expected_terms_digest")
            .filter(|v| !v.is_null())
            .map(|v| {
                let s = v.as_str().ok_or_else(invalid)?;
                if s.len() != 64
                    || !s
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(invalid());
                }
                Ok(s.to_owned())
            })
            .transpose()?;
        let expected_order_version = object
            .get("expected_order_version")
            .filter(|v| !v.is_null())
            .map(|v| v.as_i64().filter(|v| *v > 0).ok_or_else(invalid))
            .transpose()?;
        let flag = |key| {
            object
                .get(key)
                .map(|v| v.as_bool().ok_or_else(invalid))
                .unwrap_or(Ok(false))
        };
        let terms = |key| object.get(key).filter(|v| !v.is_null()).cloned();
        let normalize = |profile, terms: Option<Value>| {
            terms
                .map(|value| {
                    crate::market::terms::owner_response_terms(profile, &value)
                        .map_err(|_| invalid())
                })
                .transpose()
        };
        let fame_terms = normalize("fame_v1", terms("fame_terms"))?;
        let lng_terms = normalize("lng_v1", terms("lng_terms"))?;
        Ok(Self {
            order_id,
            quantity_mt,
            expected_terms_digest,
            expected_order_version,
            fame_terms,
            lng_terms,
            certification_declared: flag("certification_declared")?,
            msds_available: flag("msds_available")?,
        })
    }
    pub fn request_hash(&self) -> Result<String, MarketWriteError> {
        let mut value =
            json!({"order_id":self.order_id,"quantity_mt":self.quantity_mt.to_string()});
        for (key, item) in [
            (
                "expected_terms_digest",
                self.expected_terms_digest.as_ref().map(|v| json!(v)),
            ),
            (
                "expected_order_version",
                self.expected_order_version.map(|v| json!(v)),
            ),
            ("fame_terms", self.fame_terms.clone()),
            ("lng_terms", self.lng_terms.clone()),
        ] {
            if let Some(item) = item {
                value[key] = item;
            }
        }
        if self.certification_declared {
            value["certification_declared"] = json!(true);
        }
        if self.msds_available {
            value["msds_available"] = json!(true);
        }
        super::idempotency_request_hash(&Canonical::from_json(&value).map_err(|_| invalid())?)
            .map_err(|_| invalid())
    }
}
#[derive(Clone, Debug)]
pub struct TradePage {
    pub skip: i64,
    pub limit: i64,
    pub action_required: bool,
    pub status_group: String,
}
impl Default for TradePage {
    fn default() -> Self {
        Self {
            skip: 0,
            limit: 20,
            action_required: false,
            status_group: "all".into(),
        }
    }
}
impl TradePage {
    pub fn parse(raw: Option<&str>) -> Result<Self, MarketWriteError> {
        let mut p = Self::default();
        for (k, v) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            match k.as_ref() {
                "skip" => p.skip = v.parse().map_err(|_| invalid())?,
                "limit" => p.limit = v.parse().map_err(|_| invalid())?,
                "action_required" => p.action_required = v.parse().map_err(|_| invalid())?,
                "status_group" => p.status_group = v.into_owned(),
                _ => {}
            }
        }
        if p.skip < 0
            || !(1..=100).contains(&p.limit)
            || !matches!(p.status_group.as_str(), "all" | "active" | "completed")
        {
            return Err(invalid());
        }
        Ok(p)
    }
}

pub fn project_trade(
    mut row: Value,
    viewer: Option<Uuid>,
    admin: bool,
) -> Result<Value, MarketWriteError> {
    let uuid = |key| {
        row.get(key)
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<Uuid>().ok())
    };
    let buyer = uuid("buyer_id");
    let seller = uuid("seller_id");
    let is_buyer = viewer.is_some() && viewer == buyer;
    let is_seller = viewer.is_some() && viewer == seller;
    let anonymous = row["is_anonymous"].as_bool().unwrap_or(false);
    let handoff = matches!(
        row["status"].as_str(),
        Some("CONFIRMED" | "DELIVERED" | "PAID")
    );
    if anonymous {
        if !is_buyer && !is_seller {
            for side in ["buyer", "seller"] {
                row[format!("{side}_id")] = Value::Null;
                row[format!("{side}_name")] = json!("Anonymous");
            }
        } else if !handoff {
            let side = if is_buyer { "seller" } else { "buyer" };
            row[format!("{side}_id")] = Value::Null;
            row[format!("{side}_name")] = json!("Anonymous");
        }
    }
    let fee = row
        .get("commission_fee_per_mt_usd")
        .is_some_and(|v| !v.is_null());
    row["commission_payer"] = if fee { json!("SELLER") } else { Value::Null };
    if !is_seller && !admin {
        row["commission_fee_per_mt_usd"] = Value::Null;
        row["commission_plan"] = Value::Null;
        if fee {
            row["commission_amount_usd"] = Value::Null;
        }
    }
    if row["commission_rate_pct"].is_null() {
        row["commission_rate_pct"] = json!("0.5");
    }
    for (field, profile, reveal) in [
        (
            "lng_terms_snapshot",
            "lng_v1",
            (is_buyer || is_seller) && handoff,
        ),
        (
            "fame_terms_snapshot",
            "fame_v1",
            is_seller || (is_buyer && (!anonymous || handoff)),
        ),
    ] {
        if !reveal
            && let Some(ask) = row
                .get_mut(field)
                .and_then(|snapshot| snapshot.get_mut("ask"))
        {
            *ask =
                crate::market::terms::public_response_terms(profile, ask).map_err(|_| fault())?;
        }
    }
    // The response is an allowlisted DTO, never an open-ended database row.
    const FIELDS: &[&str] = &[
        "id",
        "bid_order_id",
        "ask_order_id",
        "buyer_id",
        "seller_id",
        "buyer_name",
        "seller_name",
        "initiated_by",
        "is_anonymous",
        "quantity_mt",
        "price_per_mt_usd",
        "status",
        "final_quantity_mt",
        "final_price_per_mt",
        "final_total_usd",
        "commission_rate_pct",
        "commission_fee_per_mt_usd",
        "commission_plan",
        "commission_payer",
        "commission_amount_usd",
        "confirmed_at",
        "delivered_at",
        "paid_at",
        "created_at",
        "product_id",
        "product_name",
        "market_product",
        "fuel_type",
        "fuel_grade",
        "delivery_point_id",
        "delivery_point_name",
        "region",
        "availability_window",
        "source_kind",
        "scope",
        "demo_status",
        "unknown_count",
        "lng_terms_snapshot",
        "fame_terms_snapshot",
    ];
    row.as_object_mut()
        .ok_or_else(fault)?
        .retain(|key, _| FIELDS.contains(&key.as_str()));
    Ok(row)
}

#[derive(Clone)]
pub(crate) struct TradeRequest {
    pub create: Option<TradeCreate>,
    pub key: Option<String>,
    pub context: Option<Uuid>,
    pub page: TradePage,
    pub summary: bool,
    pub audit: AuditContext,
    pub support_enabled: bool,
    pub now: DateTime<Utc>,
}
struct TradeCommand {
    request: TradeRequest,
    access: Arc<crate::auth::VerifiedAccess>,
    policy: Arc<MarketPolicy>,
    config: Arc<crate::Config>,
    hash: Option<String>,
    fees: crate::subscriptions::FeeConfig,
}
impl TradeCommand {
    async fn preview_inner(
        &self,
        conn: &mut PgConnection,
        actor: Uuid,
    ) -> Result<MarketPreview, MarketAttemptError> {
        let authenticated = super::authority::admission::authenticate_preview(
            conn,
            self.access.clone(),
            if self.request.create.is_some() {
                AdmissionClass::Execution
            } else {
                AdmissionClass::Cleanup
            },
        )
        .await?;
        let org = authenticated.account().current_organization_id;
        let mut organizations: Vec<Uuid> = org.into_iter().collect();
        let mut users = Vec::new();
        let mut facts = json!({"organization":org});
        let mut orders = Vec::new();
        let mut items = Vec::new();
        if let Some(input) = &self.request.create {
            if org.is_none() {
                return Err(
                    refusal(MarketErrorCode::ValidationFailed, StatusCode::BAD_REQUEST).into(),
                );
            }
            // A committed key needs no current target, admission or effects.
            let replay: Option<Uuid> = if let Some(key) = &self.request.key {
                sqlx::query_scalar("SELECT id FROM trades WHERE initiator_org_id=$1 AND idempotency_operation='trade.create' AND idempotency_key=$2").bind(org).bind(key).fetch_optional(&mut *conn).await?
            } else {
                None
            };
            facts["replay"] = json!(replay);
            if replay.is_none() {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orderbook_orders WHERE id=$1)")
                        .bind(input.order_id)
                        .fetch_one(&mut *conn)
                        .await?;
                if !exists {
                    return Err(MarketWriteError::new(MarketErrorCode::OrderNotFound).into());
                }
                let row = matching::load_order(conn, input.order_id).await?;
                organizations.push(row.organization_id);
                users.extend(row.owner_user_id);
                if row.creation_method == matching::CreationMethod::MarketSupport {
                    let members:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 AND role::text IN ('BUYER','SUPPLIER') ORDER BY id").bind(row.organization_id).fetch_all(&mut *conn).await?;
                    users.extend(members);
                }
                facts["target"] = json!({"id":row.id,"organization":row.organization_id,"owner":row.owner_user_id,"creator":row.created_by_actor_user_id,"product":row.product_id,"point":row.delivery_point_id,"window":row.availability_window,"item":row.inventory_item_id});
                orders.push(row.clone());
                items.extend(row.inventory_item_id);
            }
        }
        if let Some(context) = self.request.context {
            return Ok(super::support::contexts::preview_for_read(conn, actor, context).await?);
        }
        let parties = PartyLockPreview::new(organizations, users, actor, facts);
        let mut locks = OperationLockPlan::new(if self.request.create.is_some() {
            MarketOperation::Take
        } else {
            MarketOperation::InventoryEdit
        });
        locks.parties(&parties)?;
        // Only a take takes the create idempotency lock; a read never touches
        // the create advisory-key space, whatever headers it carried.
        if let (Some(org), Some(key)) = (org, self.request.key.as_deref())
            && self.request.create.is_some()
        {
            locks.idempotency(org, "trade.create", key)?;
        }
        for row in &orders {
            locks.slices([row.slice()])?;
        }
        if self.request.create.is_some() {
            locks.orders(orders.iter().map(|r| r.id))?;
            locks.inventory(items)?;
        }
        Ok(MarketPreview { parties, locks })
    }
}
impl MarketCommand for TradeCommand {
    type Output = CommittedCommand;
    fn replans_missing_lock_rows(&self) -> bool {
        true
    }
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(self.preview_inner(conn, actor))
    }
    fn replay<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
        Box::pin(async move {
            if self.request.create.is_none() {
                return Ok(None);
            }
            tx.trades()?
                .replay(
                    actor,
                    self.request.key.as_deref(),
                    self.hash.as_deref().unwrap_or_default(),
                )
                .await
                .map_err(Into::into)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            if let Some(input) = &self.request.create {
                let org = actor
                    .account()
                    .current_organization_id
                    .ok_or_else(invalid)?;
                tx.execution_context(
                    actor,
                    org,
                    self.request.now,
                    self.config.kyc_execution_gate_enabled,
                )
                .await?;
                let before = tx
                    .effects()?
                    .capture_before(
                        &self.policy,
                        &self.config,
                        &[input.order_id],
                        self.request.now,
                    )
                    .await?;
                let result = tx
                    .trades()?
                    .take(
                        actor,
                        input,
                        self.request.key.as_deref(),
                        self.hash.as_deref(),
                        &self.policy,
                        &self.config,
                        &self.fees,
                        self.request.now,
                    )
                    .await?;
                let body = result;
                tx.effects()?
                    .trade_created(
                        before,
                        actor.user_id(),
                        org,
                        &body,
                        input,
                        &self.request.audit,
                        &self.policy,
                        &self.config,
                        self.request.now,
                    )
                    .await?;
                Ok(CommittedCommand {
                    response_status: 200,
                    response_body: project_trade(body, Some(org), false)?,
                    replayed: false,
                })
            } else {
                let context = if let Some(id) = self.request.context {
                    Some(
                        tx.resolve_trade_context(
                            actor,
                            id,
                            self.request.support_enabled,
                            self.request.now,
                            &self.policy,
                            &self.config,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                if context == Some(None) {
                    return Ok(CommittedCommand {
                        response_status: 409,
                        response_body: json!({"detail":{"code":"MARKET_SUPPORT_CONTEXT_INVALID","message":"Support context has expired"}}),
                        replayed: false,
                    });
                }
                let body = tx
                    .trades()?
                    .read(
                        actor,
                        context.flatten(),
                        &self.request.page,
                        self.request.summary,
                        &self.config,
                        self.request.now,
                    )
                    .await?;
                Ok(CommittedCommand {
                    response_status: 200,
                    response_body: body,
                    replayed: false,
                })
            }
        })
    }
}
pub(crate) async fn dispatch(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: TradeRequest,
    fees: crate::subscriptions::FeeConfig,
) -> Result<CommittedCommand, MarketWriteError> {
    let hash = request
        .create
        .as_ref()
        .map(TradeCreate::request_hash)
        .transpose()?;
    let command = TradeCommand {
        request,
        access: Arc::new(crate::auth::VerifiedAccess {
            subject: access.subject,
            issued_at: access.issued_at,
            issuance_out_of_range: access.issuance_out_of_range,
        }),
        policy: app.policy,
        config: app.config,
        hash,
        fees,
    };
    let class = if command.request.create.is_some() {
        AdmissionClass::Execution
    } else {
        AdmissionClass::Cleanup
    };
    run_market_command(app.pools.read.clone(), app.reads, access, command, class).await
}

pub(super) struct TradePort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
}
impl<'a> TradePort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &'a Locked,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self { conn, locked })
    }
    async fn load(&mut self, id: Uuid) -> Result<Value, MarketWriteError> {
        let mut value:Value=sqlx::query_scalar("SELECT to_jsonb(t)||jsonb_build_object('quantity_mt',t.quantity_mt::text,'price_per_mt_usd',t.price_per_mt_usd::text,'commission_rate_pct',t.commission_rate_pct::text,'commission_fee_per_mt_usd',t.commission_fee_per_mt_usd::text,'commission_amount_usd',t.commission_amount_usd::text,'final_quantity_mt',t.final_quantity_mt::text,'final_price_per_mt',t.final_price_per_mt::text,'final_total_usd',t.final_total_usd::text,'buyer_name',COALESCE(b.name,''),'seller_name',COALESCE(s.name,''),'region',COALESCE(t.delivery_point_region,'')) FROM trades t LEFT JOIN organizations b ON b.id=t.buyer_id LEFT JOIN organizations s ON s.id=t.seller_id WHERE t.id=$1").bind(id).fetch_one(&mut *self.conn).await?;
        let buyer = value["buyer_provenance"].as_str().unwrap_or("UNKNOWN");
        let seller = value["seller_provenance"].as_str().unwrap_or("UNKNOWN");
        let real = buyer == "REAL" && seller == "REAL";
        let demo = buyer == "DEMO" && seller == "DEMO";
        let unknown = buyer == "UNKNOWN" || seller == "UNKNOWN";
        value["source_kind"] = json!(if real {
            "CONFIRMED_TRADE"
        } else if demo {
            "DEMO_SEED"
        } else {
            "UNKNOWN"
        });
        value["scope"] = json!(if value["delivery_point_id"].is_null() {
            "UNKNOWN"
        } else {
            "DELIVERY_POINT"
        });
        value["demo_status"] = json!(if real {
            "REAL_ONLY"
        } else if demo {
            "DEMO_ONLY"
        } else {
            "UNKNOWN"
        });
        value["unknown_count"] = json!(u32::from(unknown));
        Ok(value)
    }
    async fn replay(
        &mut self,
        actor: &AuthenticatedActor,
        key: Option<&str>,
        hash: &str,
    ) -> Result<Option<CommittedCommand>, MarketWriteError> {
        let Some(key) = key else {
            return Ok(None);
        };
        let org = actor.account().current_organization_id;
        let row=sqlx::query("SELECT id,idempotency_request_hash,CASE WHEN initiated_by::text='BUYER' THEN buyer_user_id ELSE seller_user_id END AS actor FROM trades WHERE initiator_org_id=$1 AND idempotency_operation='trade.create' AND idempotency_key=$2").bind(org).bind(key).fetch_optional(&mut *self.conn).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.get::<Option<Uuid>, _>("actor") != Some(actor.user_id())
            || row
                .get::<Option<String>, _>("idempotency_request_hash")
                .as_deref()
                != Some(hash)
        {
            return Err(MarketWriteError::new(MarketErrorCode::IdempotencyConflict));
        }
        let body = self.load(row.get("id")).await?;
        Ok(Some(CommittedCommand {
            response_status: 200,
            response_body: project_trade(body, org, actor.account().role == ActorRole::Admin)?,
            replayed: true,
        }))
    }
    #[allow(clippy::too_many_arguments)]
    async fn take(
        &mut self,
        actor: &AuthenticatedActor,
        input: &TradeCreate,
        key: Option<&str>,
        hash: Option<&str>,
        policy: &MarketPolicy,
        config: &crate::Config,
        fees: &crate::subscriptions::FeeConfig,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        if !self.locked.contains(LockClass::Order, input.order_id) {
            return Err(unavailable());
        }
        let order = matching::load_order(self.conn, input.order_id).await?;
        let raw: Value =
            sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
                .bind(order.id)
                .fetch_one(&mut *self.conn)
                .await?;
        let digest = crate::canonical_terms_from_row(&raw)
            .and_then(|v| crate::digest_terms(&v))
            .map_err(|_| fault())?;
        if input.expected_terms_digest.as_deref() != Some(&digest) {
            return Err(refusal(
                MarketErrorCode::OrderTermsReviewRequired,
                StatusCode::CONFLICT,
            ));
        }
        let catalog = policy
            .products
            .iter()
            .find(|p| {
                p.id == order.product_id
                    && p.execution_mode == "ORDERBOOK"
                    && order
                        .delivery_point_id
                        .is_some_and(|id| p.lanes.contains(&id))
            })
            .ok_or_else(unavailable)?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products p JOIN delivery_points d ON d.id=$2 WHERE p.id=$1 AND p.is_active AND d.is_active)").bind(order.product_id).bind(order.delivery_point_id).fetch_one(&mut *self.conn).await?;
        let bound = MatchBound::new(now)
            .with_policy(policy)
            .with_kyc_gate(config.kyc_execution_gate_enabled);
        if !active || !order.live_at(now) || !matching::order_is_execution_qualified(&order, bound)
        {
            return Err(unavailable());
        }
        if [
            matching::DEMO_ACTIVITY_BUYER_ORG_ID,
            matching::DEMO_ACTIVITY_SELLER_ORG_ID,
        ]
        .contains(&order.organization_id)
        {
            return Err(unavailable());
        }
        if order.owner_user_id.is_none() {
            return Err(refusal(
                MarketErrorCode::TradePartiesInvalid,
                StatusCode::CONFLICT,
            ));
        }
        if matches!(
            order.execution_provenance(),
            matching::Provenance::Demo | matching::Provenance::Test | matching::Provenance::Canary
        ) {
            return Err(unavailable());
        }
        if order.side == Side::Ask && actor.account().role != ActorRole::Buyer {
            return Err(MarketWriteError::new(MarketErrorCode::TradePartyForbidden));
        }
        if order.side == Side::Bid && actor.account().role != ActorRole::Supplier {
            return Err(MarketWriteError::new(MarketErrorCode::TradePartyForbidden));
        }
        if order.side == Side::Bid && catalog.specification_standard.is_some() {
            return Err(unavailable());
        }
        let org = actor
            .account()
            .current_organization_id
            .ok_or_else(invalid)?;
        if org == order.organization_id {
            return Err(unavailable());
        }
        let provenance: String =
            sqlx::query_scalar("SELECT provenance::text FROM organizations WHERE id=$1")
                .bind(org)
                .fetch_one(&mut *self.conn)
                .await?;
        if provenance != "REAL" || order.execution_provenance() != matching::Provenance::Real {
            return Err(unavailable());
        }
        if matching::OwnerRevalidation::default()
            .revalidate(self.conn, self.locked, &order, &bound)
            .await?
            != OwnerAdmission::Admitted
        {
            return Err(unavailable());
        }
        if input.quantity_mt > order.remaining_quantity_mt {
            return Err(refusal(
                MarketErrorCode::QuantityExceedsRemaining,
                StatusCode::BAD_REQUEST,
            ));
        }
        let (fame, lng) = counterparty_snapshot(&order, input, catalog, now)?;
        let (buyer, seller, buyer_user, seller_user, initiated) = if order.side == Side::Ask {
            (
                org,
                order.organization_id,
                Some(actor.user_id()),
                order.owner_user_id,
                "BUYER",
            )
        } else {
            (
                order.organization_id,
                org,
                order.owner_user_id,
                Some(actor.user_id()),
                "SELLER",
            )
        };
        let fee = crate::subscriptions::resolve_seller_fee(self.conn, seller, now, fees)
            .await
            .map_err(|e| match e {
                crate::subscriptions::FeeError::Database(e) => e.into(),
                _ => refusal(MarketErrorCode::FeeRateInvalid, StatusCode::CONFLICT),
            })?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO trades(id,bid_order_id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,buyer_provenance,seller_provenance,initiated_by,quantity_mt,price_per_mt_usd,status,is_anonymous,idempotency_key,idempotency_operation,idempotency_request_hash,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version,commission_rate_pct,commission_fee_per_mt_usd,commission_plan,fame_terms_snapshot,lng_terms_snapshot,created_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,'REAL','REAL',$9,$10,o.price_per_mt_usd,'PENDING_CONFIRMATION',true,$11,CASE WHEN $11::text IS NULL THEN NULL ELSE 'trade.create' END,$12,o.product_id,p.name,p.fuel_type,p.fuel_grade,$13,o.delivery_point_id,d.name,d.region,o.availability_window,1,0,$14,$15,$16,$17,$18 FROM orderbook_orders o JOIN products p ON p.id=o.product_id JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.id=$19")
   .bind(id).bind((order.side==Side::Bid).then_some(order.id)).bind((order.side==Side::Ask).then_some(order.id)).bind(buyer).bind(seller).bind(buyer_user).bind(seller_user).bind(org).bind(initiated).bind(input.quantity_mt).bind(key).bind(if key.is_some(){hash}else{None}).bind(&catalog.code).bind(fee.seller_fee_per_mt_usd).bind(fee.plan.as_str()).bind(fame).bind(lng).bind(now).bind(order.id).execute(&mut *self.conn).await?;
        sqlx::query("UPDATE orderbook_orders SET remaining_quantity_mt=remaining_quantity_mt-$2,status=CASE WHEN remaining_quantity_mt=$2 THEN 'FILLED' WHEN status='OPEN' THEN 'PARTIALLY_FILLED' ELSE status END,version=version+1,updated_at=$3 WHERE id=$1").bind(order.id).bind(input.quantity_mt).bind(now).execute(&mut *self.conn).await?;
        self.load(id).await
    }
}
fn counterparty_snapshot(
    order: &matching::OrderRow,
    input: &TradeCreate,
    product: &crate::ProductPolicy,
    now: DateTime<Utc>,
) -> Result<(Option<Value>, Option<Value>), MarketWriteError> {
    let bid_side = order.side == Side::Bid;
    let profile = product.qualification_profile.as_str();
    if profile == "fame_v1" && bid_side && !(input.certification_declared && input.msds_available) {
        return Err(invalid());
    }
    if profile == "lng_v1" && bid_side && !input.msds_available {
        return Err(invalid());
    }
    let (stored, supplied) = match profile {
        "fame_v1" => (order.terms.fame_terms.as_ref(), input.fame_terms.as_ref()),
        "lng_v1" => (order.terms.lng_terms.as_ref(), input.lng_terms.as_ref()),
        _ => {
            if input.fame_terms.is_some() || input.lng_terms.is_some() {
                return Err(invalid());
            }
            return Ok((None, None));
        }
    };
    let stored = stored.ok_or_else(invalid)?;
    let supplied = supplied.ok_or_else(invalid)?;
    // The taker fills the resting order's opposite side. A declaration of the
    // resting side is invalid input, whatever its own field validity, and is
    // refused before any pairing or compatibility rule can normalize it into
    // the wrong slot.
    let expected = if bid_side { "ASK" } else { "BID" };
    if supplied.get("side").and_then(Value::as_str) != Some(expected) {
        return Err(invalid());
    }
    let (bid, ask) = if bid_side {
        (stored, supplied)
    } else {
        (supplied, stored)
    };
    let compatible = if profile == "fame_v1" {
        matching::fame_terms_compatible(
            bid,
            ask,
            now.with_timezone(&chrono_tz::Asia::Singapore).date_naive(),
        )
    } else {
        matching::lng_terms_compatible(bid, ask)
    };
    if !compatible {
        let mut e = invalid();
        if profile == "lng_v1" {
            e = MarketWriteError::new(MarketErrorCode::LngTermsIncompatible);
        }
        return Err(e);
    }
    let bid = crate::market::terms::owner_response_terms(profile, bid).map_err(|_| invalid())?;
    let ask = crate::market::terms::owner_response_terms(profile, ask).map_err(|_| invalid())?;
    if profile == "fame_v1" {
        Ok((Some(json!({"schema_version":1,"bid":bid,"ask":ask})), None))
    } else {
        Ok((None, Some(json!({"bid":bid,"ask":ask}))))
    }
}

impl TradePort<'_> {
    async fn read(
        &mut self,
        actor: &AuthenticatedActor,
        supported: Option<Uuid>,
        page: &TradePage,
        summary: bool,
        config: &crate::Config,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        let kyc = crate::kyc::policy::eligibility_sql(crate::kyc::policy::KycSqlParty::USER)
            .replace("/*kyc_enabled*/", "$4")
            .replace("/*kyc_now*/", "$5");
        let relation = format!(
            r#"WITH caller AS (
   SELECT u.*,o.verification_status::text AS organization_status,({kyc}) AS kyc_eligible FROM users u LEFT JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1
  ), authorized AS (
   SELECT t.id,t.status,t.created_at,
    CASE WHEN $2::uuid IS NOT NULL THEN $2 WHEN t.buyer_user_id=$1 AND COALESCE(bo.creation_method::text,'')<>'MARKET_SUPPORT' THEN t.buyer_id WHEN t.seller_user_id=$1 AND COALESCE(so.creation_method::text,'')<>'MARKET_SUPPORT' THEN t.seller_id ELSE u.organization_id END AS viewer,
    COALESCE(t.initiator_org_id,CASE WHEN t.initiated_by::text='BUYER' THEN t.buyer_id ELSE t.seller_id END) AS initiator,
    t.buyer_id,t.seller_id,t.buyer_user_id,t.seller_user_id,
    u.organization_id,u.role::text AS role,u.status::text='APPROVED' AND u.email_verified AND NOT u.must_change_password AND u.organization_status='APPROVED' AND u.kyc_eligible AS executable,
    (bo.creation_method::text='MARKET_SUPPORT' AND bo.side::text='BID' AND bo.organization_id=t.buyer_id AND bo.owner_user_id=t.buyer_user_id AND bo.created_by_actor_user_id=bo.owner_user_id) AS buyer_assisted,
    (so.creation_method::text='MARKET_SUPPORT' AND so.side::text='ASK' AND so.organization_id=t.seller_id AND so.owner_user_id=t.seller_user_id AND so.created_by_actor_user_id=so.owner_user_id) AS seller_assisted
   FROM trades t CROSS JOIN caller u LEFT JOIN orderbook_orders bo ON bo.id=t.bid_order_id LEFT JOIN orderbook_orders so ON so.id=t.ask_order_id
   WHERE CASE WHEN $2::uuid IS NOT NULL THEN t.buyer_id=$2 OR t.seller_id=$2 ELSE
    (u.status::text='APPROVED' AND u.organization_status='APPROVED' AND (t.buyer_id=u.organization_id OR t.seller_id=u.organization_id))
    OR (t.buyer_user_id=$1 AND COALESCE(bo.creation_method::text,'')<>'MARKET_SUPPORT')
    OR (t.seller_user_id=$1 AND COALESCE(so.creation_method::text,'')<>'MARKET_SUPPORT')
    OR (u.role::text='BUYER' AND u.organization_id=t.buyer_id AND bo.creation_method::text='MARKET_SUPPORT' AND bo.side::text='BID' AND bo.organization_id=t.buyer_id AND bo.owner_user_id=t.buyer_user_id AND bo.created_by_actor_user_id=bo.owner_user_id)
    OR (u.role::text='SUPPLIER' AND u.organization_id=t.seller_id AND so.creation_method::text='MARKET_SUPPORT' AND so.side::text='ASK' AND so.organization_id=t.seller_id AND so.owner_user_id=t.seller_user_id AND so.created_by_actor_user_id=so.owner_user_id) END
  ), flags AS (
   SELECT *,COALESCE(status::text='PENDING_CONFIRMATION' AND executable AND organization_id=viewer AND initiator<>viewer AND
    ((viewer=buyer_id AND role='BUYER' AND (buyer_user_id=$1 OR buyer_assisted)) OR (viewer=seller_id AND role='SUPPLIER' AND (seller_user_id=$1 OR seller_assisted))),false) AS action,
    status::text='PENDING_CONFIRMATION' AND initiator=viewer AS awaiting
   FROM authorized
  ), selected AS (SELECT * FROM flags WHERE ($3::text='all' OR ($3='active' AND status::text='PENDING_CONFIRMATION') OR ($3='completed' AND status::text IN ('CONFIRMED','DELIVERED','PAID'))) AND (NOT $6 OR action)) "#
        );
        if summary {
            let sql = format!(
                "{relation} SELECT count(*) AS total, count(*) FILTER(WHERE action) AS action,count(*) FILTER(WHERE awaiting) AS awaiting,count(*) FILTER(WHERE status::text IN ('CONFIRMED','DELIVERED','PAID')) AS confirmed FROM flags"
            );
            let row = sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(actor.user_id())
                .bind(supported)
                .bind("all")
                .bind(config.kyc_execution_gate_enabled)
                .bind(now)
                .bind(false)
                .fetch_one(&mut *self.conn)
                .await?;
            return Ok(
                json!({"total_count":row.get::<i64,_>("total"),"action_required_count":row.get::<i64,_>("action"),"awaiting_counterparty_count":row.get::<i64,_>("awaiting"),"confirmed_count":row.get::<i64,_>("confirmed")}),
            );
        }
        let sql = format!(
            "{relation} SELECT (SELECT count(*) FROM selected) AS total,COALESCE((SELECT jsonb_agg(jsonb_build_object('id',p.id,'viewer',p.viewer) ORDER BY p.created_at DESC,p.id DESC) FROM (SELECT id,viewer,created_at FROM selected ORDER BY created_at DESC,id DESC OFFSET $7 LIMIT $8) p),'[]'::jsonb) AS entries"
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(actor.user_id())
            .bind(supported)
            .bind(&page.status_group)
            .bind(config.kyc_execution_gate_enabled)
            .bind(now)
            .bind(page.action_required)
            .bind(page.skip)
            .bind(page.limit)
            .fetch_one(&mut *self.conn)
            .await?;
        let entries: Value = row.get("entries");
        let mut items = Vec::new();
        for entry in entries.as_array().ok_or_else(fault)? {
            let id = entry["id"]
                .as_str()
                .and_then(|v| v.parse().ok())
                .ok_or_else(fault)?;
            let viewer = entry["viewer"].as_str().and_then(|v| v.parse().ok());
            let value = self.load(id).await?;
            items.push(project_trade(
                value,
                viewer,
                actor.account().role == ActorRole::Admin,
            )?);
        }
        Ok(
            json!({"items":items,"total":row.get::<i64,_>("total"),"skip":page.skip,"limit":page.limit}),
        )
    }
}

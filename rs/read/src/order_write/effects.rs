//! Caller-owned audit, notification, benchmark, PIN display and outbox effects.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::locks::Locked;
use super::outbox::{ParticipantEvent, ProducerEvidence};
use super::{ExpiryEffect, MarketErrorCode, MarketWriteError};
use crate::business::audit::{AuditFact, append_audit};
use crate::market::live_benchmarks::{candidate_is_benchmark_qualified, qualified_vwap};
use crate::market::order_projection::PublicOrder;
use crate::market::visibility::{
    OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
};
use crate::market::windows::normalize_window;
use crate::{Config, MarketPolicy};

#[derive(Clone, Copy, Debug)]
pub enum MutationKind {
    OrderCreated,
    AssistedOrderAuthorizationCreated,
    OrderCancelled,
    OrderExpired,
    OrderUpdated,
    InventoryCreated,
    InventoryUpdated,
    InventoryDeleted,
    InventoryPublished,
    SupportContextStarted,
    SupportContextExited,
    SupportContextRevoked,
    SupportAuthorizationCreated,
    SupportAuthorizationRevoked,
    SupportCapabilityGranted,
    SupportCapabilityRevoked,
    CommissionChanged,
    TradeCreated,
    TradeDelivered,
    TradePaid,
    TradeConfirmed,
    TradeDeclined,
    TradeDeclineOrderExpired,
    TradeAutoMatched,
    OrdersMatched,
}

impl MutationKind {
    pub fn audit_action(self) -> Option<&'static str> {
        Some(match self {
            Self::InventoryCreated | Self::InventoryUpdated | Self::InventoryDeleted => {
                return None;
            }
            Self::OrderCreated => "order.created",
            Self::AssistedOrderAuthorizationCreated => "market_support.authorization_created",
            Self::OrderCancelled => "order.cancelled",
            Self::OrderExpired => "order.expired",
            Self::OrderUpdated => "order.updated",
            Self::InventoryPublished => "inventory.published",
            Self::SupportContextStarted => "market_support.context_started",
            Self::SupportContextExited => "market_support.context_exited",
            Self::SupportContextRevoked => "market_support.context_revoked",
            Self::SupportAuthorizationCreated | Self::SupportAuthorizationRevoked => return None,
            Self::SupportCapabilityGranted => "market_support.capability_granted",
            Self::SupportCapabilityRevoked => "market_support.capability_revoked",
            Self::CommissionChanged => "commission.updated",
            Self::TradeCreated => "trade.created",
            Self::TradeDelivered => "trade.delivered",
            Self::TradePaid => "trade.paid",
            Self::TradeConfirmed => "trade.confirmed",
            Self::TradeDeclined => "trade.declined",
            Self::TradeDeclineOrderExpired => "order.expired",
            Self::TradeAutoMatched => "trade.auto_matched",
            Self::OrdersMatched => return None,
        })
    }
    fn changes_order_economics(self) -> bool {
        matches!(
            self,
            Self::OrderCreated
                | Self::OrderCancelled
                | Self::OrderExpired
                | Self::OrderUpdated
                | Self::InventoryPublished
                | Self::TradeCreated
                | Self::TradeDeclined
                | Self::TradeDeclineOrderExpired
                | Self::TradeAutoMatched
        )
    }
    fn is_retired(self) -> bool {
        matches!(
            self,
            Self::SupportAuthorizationCreated | Self::SupportAuthorizationRevoked
        )
    }
}

pub struct MutationEffect {
    pub kind: MutationKind,
    pub actor_id: Uuid,
    pub organization_id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub changes: Value,
    pub event: Option<ParticipantEvent>,
    pub at: DateTime<Utc>,
    pub audit_context: AuditContext,
}

/// Bounded request metadata carried into the existing audit columns.
#[derive(Clone, Debug, Default)]
pub struct AuditContext {
    ip_address: Option<String>,
    request_id: Option<String>,
}
impl AuditContext {
    pub fn new(
        ip_address: Option<String>,
        request_id: Option<String>,
    ) -> Result<Self, MarketWriteError> {
        if ip_address
            .as_ref()
            .is_some_and(|s| s.chars().count() > 45 || s.chars().any(char::is_control))
            || request_id
                .as_ref()
                .is_some_and(|s| s.chars().count() > 36 || s.chars().any(char::is_control))
        {
            return Err(MarketWriteError::new(MarketErrorCode::ValidationFailed));
        }
        Ok(Self {
            ip_address,
            request_id,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct EffectSlice {
    pub product_id: Uuid,
    pub delivery_point_id: Uuid,
    pub availability_window: String,
}
/// A caller-declared insertion under an already held organization and slice.
#[derive(Clone, Debug)]
pub struct NewOrderEffect {
    pub order_id: Uuid,
    pub organization_id: Uuid,
    pub slice: EffectSlice,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Slice {
    product_id: Uuid,
    product: String,
    point: Uuid,
    window: String,
}

struct OrderState {
    id: Uuid,
    organization_id: Uuid,
    product_id: Uuid,
    /// The market policy code; absent for a product outside the policy.
    product: Option<String>,
    /// Absent for an order without a delivery point.
    point: Option<Uuid>,
    window: String,
    version: i32,
    status: String,
    side: String,
    price: Decimal,
    quantity: Decimal,
    remaining: Decimal,
    point_name: Option<String>,
    public: Option<Value>,
    qualified: bool,
    provenance: String,
}

impl OrderState {
    /// The benchmark slice. An order without a policy product or a delivery
    /// point has none, so no benchmark aggregate or rebuild covers it.
    fn slice(&self) -> Option<Slice> {
        Some(Slice {
            product_id: self.product_id,
            product: self.product.clone()?,
            point: self.point?,
            window: self.window.clone(),
        })
    }
    fn same_market_identity(&self, other: &Self) -> bool {
        self.id == other.id
            && self.product_id == other.product_id
            && self.point == other.point
            && self.window == other.window
    }
    fn evidence(&self) -> Value {
        json!({"order_id":self.id,"version":self.version,"status":self.status,
            "side":self.side,"product_id":self.product_id,"market_product":self.product,
            "delivery_point_id":self.point,"availability_window":self.window,
            "price_per_mt_usd":self.price.to_string(),"quantity_mt":self.quantity.to_string(),
            "remaining_quantity_mt":self.remaining.to_string(),"provenance":self.provenance,
            "public_admitted":self.public.is_some(),"benchmark_qualified":self.qualified,
            "public_projection":self.public})
    }
}

/// Captured before mutation while the complete economic lock graph is held.
/// It cannot be constructed from caller JSON or a consumer-time target lookup.
pub struct BeforeEffects {
    orders: Vec<OrderState>,
    new_orders: Vec<NewOrderEffect>,
    slices: BTreeSet<Slice>,
    aggregates: Vec<Value>,
    targets: Vec<Value>,
    at: DateTime<Utc>,
}

/// All descriptors for one economic transaction are staged before finalization.
pub struct EffectSet {
    before: BeforeEffects,
    effects: Vec<MutationEffect>,
    expiry: Vec<ExpiryEffect>,
    confirmed_notifications: Vec<(Uuid, Uuid, Uuid)>,
}
impl BeforeEffects {
    pub fn stage(self) -> EffectSet {
        EffectSet {
            before: self,
            effects: Vec::new(),
            expiry: Vec::new(),
            confirmed_notifications: Vec::new(),
        }
    }
}
impl EffectSet {
    pub fn push(&mut self, effect: MutationEffect) -> Result<(), MarketWriteError> {
        if effect.at != self.before.at || effect.kind.is_retired() {
            return Err(inconsistent());
        }
        self.effects.push(effect);
        Ok(())
    }
    pub fn expired(
        &mut self,
        facts: Vec<ExpiryEffect>,
        effects: Vec<MutationEffect>,
    ) -> Result<(), MarketWriteError> {
        if facts.len() != effects.len() {
            return Err(inconsistent());
        }
        for fact in &facts {
            if !self.before.orders.iter().any(|o| {
                o.id == fact.order_id
                    && o.version == fact.previous_version
                    && o.status == fact.previous_status
                    && o.remaining == fact.remaining_quantity_mt.value()
                    && fact.previous_version.checked_add(1) == Some(fact.version)
            }) || effects
                .iter()
                .filter(|e| {
                    matches!(e.kind, MutationKind::OrderExpired)
                        && e.resource_id == fact.order_id
                        && e.organization_id == fact.organization_id
                })
                .count()
                != 1
            {
                return Err(inconsistent());
            }
        }
        for effect in effects {
            self.push(effect)?;
        }
        self.expiry.extend(facts);
        Ok(())
    }
}

/// Borrowed only from the ordered transaction after all economic locks.
/// No commit, new economic lock or raw connection escapes this port.
pub struct EffectPort<'a> {
    conn: &'a mut PgConnection,
    order_ids: &'a [Uuid],
    locked: &'a Locked,
}

impl<'a> EffectPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        order_ids: &'a [Uuid],
        locked: &'a Locked,
    ) -> Self {
        Self {
            conn,
            order_ids,
            locked,
        }
    }

    /// Capture an expiring row's last live public projection while retaining
    /// the command clock for transaction effects and aggregate snapshots.
    pub async fn capture_expiry_visibility(
        &mut self,
        before: &mut BeforeEffects,
        policy: &MarketPolicy,
        config: &Config,
        facts: &[(Uuid, DateTime<Utc>)],
    ) -> Result<BTreeMap<Uuid, bool>, MarketWriteError> {
        self.locked.require_economic_phase()?;
        let mut visibility = BTreeMap::new();
        for (id, expiry) in facts {
            if !self.order_ids.contains(id) {
                return Err(inconsistent());
            }
            let at = expiry
                .checked_sub_signed(chrono::Duration::microseconds(1))
                .ok_or_else(inconsistent)?;
            let live = load_orders(self.conn, policy, config, &[*id], at)
                .await?
                .pop()
                .ok_or_else(inconsistent)?;
            let captured = before
                .orders
                .iter_mut()
                .find(|row| row.id == *id)
                .ok_or_else(inconsistent)?;
            visibility.insert(*id, live.public.is_some());
            captured.public = live.public;
            captured.qualified = live.qualified;
        }
        Ok(visibility)
    }
    pub async fn capture_before(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        ids: &[Uuid],
        at: DateTime<Utc>,
    ) -> Result<BeforeEffects, MarketWriteError> {
        self.capture_mutation(policy, config, ids, &[], &[], at)
            .await
    }

    pub async fn capture_mutation(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        ids: &[Uuid],
        declared_slices: &[EffectSlice],
        new_orders: &[NewOrderEffect],
        at: DateTime<Utc>,
    ) -> Result<BeforeEffects, MarketWriteError> {
        self.locked.require_economic_phase()?;
        if ids.iter().any(|id| !self.order_ids.contains(id)) {
            return Err(inconsistent());
        }
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let orders = load_orders(self.conn, policy, config, &ids, at).await?;
        let mut slices = slices(&orders);
        for declared in declared_slices
            .iter()
            .chain(new_orders.iter().map(|n| &n.slice))
        {
            self.locked.require_effect_slice(&super::locks::SliceKey {
                product_id: declared.product_id,
                delivery_point_id: Some(declared.delivery_point_id),
                availability_window: declared.availability_window.clone(),
            })?;
            let product = policy
                .products
                .iter()
                .find(|p| p.id == declared.product_id)
                .ok_or_else(inconsistent)?;
            let window = normalize_window(&declared.availability_window, at.date_naive())
                .map_err(|_| inconsistent())?;
            if window != declared.availability_window {
                return Err(inconsistent());
            }
            slices.insert(Slice {
                product_id: declared.product_id,
                product: product.code.clone(),
                point: declared.delivery_point_id,
                window,
            });
        }
        let mut all_ids = ids.clone();
        for new in new_orders {
            self.locked
                .require_effect_organization(new.organization_id)?;
            if all_ids.contains(&new.order_id) {
                return Err(inconsistent());
            }
            all_ids.push(new.order_id);
        }
        let new_ids: Vec<_> = new_orders.iter().map(|n| n.order_id).collect();
        let existing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE id=ANY($1)")
                .bind(&new_ids)
                .fetch_one(&mut *self.conn)
                .await?;
        if existing != 0 {
            return Err(inconsistent());
        }
        let aggregates = aggregates(self.conn, policy, config, &slices, at).await?;
        let slice_json = slice_values(&slices);
        let rows=sqlx::query("SELECT t.id,t.watchlist_id,w.user_id,t.target_type,t.order_id,t.market_product_code,t.delivery_point_id,t.availability_window_code FROM watchlist_targets t JOIN watchlists w ON w.id=t.watchlist_id WHERE (t.target_type='PIN' AND t.order_id=ANY($1)) OR (t.target_type='SLICE' AND EXISTS (SELECT 1 FROM jsonb_to_recordset($2::jsonb) AS s(product text,point uuid,window_code text) WHERE s.product=t.market_product_code AND s.point=t.delivery_point_id AND s.window_code=t.availability_window_code)) ORDER BY t.id")
            .bind(&all_ids).bind(slice_json).fetch_all(&mut *self.conn).await?;
        let targets=rows.iter().map(|r|->Result<Value,sqlx::Error>{Ok(json!({"target_id":r.try_get::<Uuid,_>("id")?,"watchlist_id":r.try_get::<Uuid,_>("watchlist_id")?,"observer_id":r.try_get::<Uuid,_>("user_id")?,"target_type":r.try_get::<String,_>("target_type")?,"order_id":r.try_get::<Option<Uuid>,_>("order_id")?,"market_product":r.try_get::<String,_>("market_product_code")?,"delivery_point_id":r.try_get::<Uuid,_>("delivery_point_id")?,"availability_window":r.try_get::<String,_>("availability_window_code")?}))}).collect::<Result<Vec<_>,_>>()?;
        Ok(BeforeEffects {
            orders,
            new_orders: new_orders.to_vec(),
            slices,
            aggregates,
            targets,
            at,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn trade_settlement(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        trade: &Value,
        request: &super::trade_lifecycle::SettlementRequest,
        actor: Uuid,
        viewer: Uuid,
        at: DateTime<Utc>,
    ) -> Result<Uuid, MarketWriteError> {
        use super::outbox::{MatchActivity, ParticipantFields};
        let buyer = trade["buyer_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(internal)?;
        let seller = trade["seller_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(internal)?;
        let delivery = request.delivery.is_some();
        let recipient_org = if delivery && viewer == buyer {
            seller
        } else {
            buyer
        };
        let recipients: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
                .bind(recipient_org)
                .fetch_all(&mut *self.conn)
                .await?;
        let text = |key: &str| trade[key].as_str().unwrap_or_default().to_owned();
        let message = if delivery {
            format!(
                "Trade for {} MT has been marked as delivered.",
                text("final_quantity_mt")
            )
        } else {
            format!(
                "Payment for trade of {} MT has been confirmed.",
                text("quantity_mt")
            )
        };
        for recipient in recipients {
            sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) VALUES($1,$2,'ORDER_UPDATE',$3,$4,$5,false,$6)")
                .bind(Uuid::new_v4()).bind(recipient).bind(if delivery {"Trade Delivered"}else{"Trade Paid"}).bind(&message).bind(json!({"trade_id":request.id})).bind(at).execute(&mut *self.conn).await?;
        }
        let activity = MatchActivity {
            source_kind: text("source_kind"),
            scope: text("scope"),
            demo_status: text("demo_status"),
            unknown_count: trade["unknown_count"].as_u64().unwrap_or(0) as u32,
            market_product: trade["market_product"].as_str().map(str::to_owned),
            delivery_point_id: trade["delivery_point_id"]
                .as_str()
                .and_then(|s| s.parse().ok()),
            availability_window: text("availability_window"),
            observed_at: trade[if delivery { "delivered_at" } else { "paid_at" }]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(internal)?,
        };
        let fields = if delivery {
            ParticipantFields::TradeDelivered {
                activity,
                id: request.id,
                final_quantity: text("final_quantity_mt"),
                final_price: text("final_price_per_mt"),
                final_total: text("final_total_usd"),
            }
        } else {
            ParticipantFields::TradePaid {
                activity,
                id: request.id,
                quantity: text("quantity_mt"),
                price: text("price_per_mt_usd"),
            }
        };
        let before = self.capture_before(policy, config, &[], at).await?;
        let mut set = before.stage();
        set.push(MutationEffect {
            kind:if delivery {MutationKind::TradeDelivered}else{MutationKind::TradePaid},actor_id:actor,organization_id:viewer,resource_type:"trade".into(),resource_id:request.id,
            changes:if delivery {json!({"status":"DELIVERED","final_quantity_mt":text("final_quantity_mt"),"final_price_per_mt":text("final_price_per_mt"),"final_total_usd":text("final_total_usd")})}else{json!({"status":"PAID"})},
            event:Some(ParticipantEvent{event_type:if delivery {"trade_delivered"}else{"trade_paid"}.into(),aggregate_type:"trade".into(),aggregate_id:request.id.to_string(),participant_org_ids:vec![buyer,seller],fields,public_invalidated:false}),at,audit_context:request.audit.clone(),
        })?;
        self.finish(policy, config, set).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn trade_lifecycle(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        outcome: super::trade_lifecycle::LifecycleOutcome,
        request: &super::trade_lifecycle::LifecycleRequest,
        actor: Uuid,
        at: DateTime<Utc>,
    ) -> Result<Uuid, MarketWriteError> {
        use super::{
            outbox::{MatchActivity, ParticipantFields},
            trade_lifecycle::LifecycleAction,
        };
        let confirm = request.action == LifecycleAction::Confirm;
        let trade = &outcome.trade;
        let parse = |key: &str| {
            trade[key]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
                .ok_or_else(internal)
        };
        let buyer = parse("buyer_id")?;
        let seller = parse("seller_id")?;
        notify_trade_initiator(self.conn, outcome.initiator, request.id, trade, confirm, at)
            .await?;
        let observed = trade[if confirm {
            "confirmed_at"
        } else {
            "created_at"
        }]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(at);
        let activity = MatchActivity {
            source_kind: trade["source_kind"].as_str().unwrap_or("UNKNOWN").into(),
            scope: trade["scope"].as_str().unwrap_or("UNKNOWN").into(),
            demo_status: trade["demo_status"].as_str().unwrap_or("UNKNOWN").into(),
            unknown_count: trade["unknown_count"].as_u64().unwrap_or(0) as u32,
            market_product: trade["market_product"].as_str().map(str::to_owned),
            delivery_point_id: trade["delivery_point_id"]
                .as_str()
                .and_then(|s| s.parse().ok()),
            availability_window: trade["availability_window"]
                .as_str()
                .unwrap_or_default()
                .into(),
            observed_at: observed,
        };
        let fields = if confirm {
            ParticipantFields::TradeConfirmed {
                activity,
                id: request.id,
                quantity: trade["quantity_mt"].as_str().unwrap_or_default().into(),
                price: trade["price_per_mt_usd"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            }
        } else {
            ParticipantFields::TradeDeclined {
                activity,
                id: request.id,
            }
        };
        let mut set = outcome.before.stage();
        if let (Some(order), Some(remainder)) = (outcome.changed_order, outcome.expired_remainder) {
            set.push(MutationEffect {
                kind: MutationKind::TradeDeclineOrderExpired,
                actor_id: actor,
                organization_id: outcome.viewer,
                resource_type: "order".into(),
                resource_id: order,
                changes: json!({"status":"EXPIRED","released_quantity_mt":remainder.to_string()}),
                event: None,
                at,
                audit_context: request.audit.clone(),
            })?;
        }
        set.push(MutationEffect {
            kind: if confirm {
                MutationKind::TradeConfirmed
            } else {
                MutationKind::TradeDeclined
            },
            actor_id: actor,
            organization_id: outcome.viewer,
            resource_type: "trade".into(),
            resource_id: request.id,
            changes: json!({"status":request.action.status()}),
            event: Some(ParticipantEvent {
                event_type: if confirm {
                    "trade_confirmed"
                } else {
                    "trade_declined"
                }
                .into(),
                aggregate_type: "trade".into(),
                aggregate_id: request.id.to_string(),
                participant_org_ids: vec![buyer, seller],
                fields,
                public_invalidated: outcome.public_invalidated,
            }),
            at,
            audit_context: request.audit.clone(),
        })?;
        self.finish(policy, config, set).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn trade_created(
        &mut self,
        before: BeforeEffects,
        actor: Uuid,
        organization: Uuid,
        trade: &Value,
        input: &super::trades::TradeCreate,
        audit: &AuditContext,
        policy: &MarketPolicy,
        config: &Config,
        at: DateTime<Utc>,
    ) -> Result<Uuid, MarketWriteError> {
        let parse = |key| {
            trade[key]
                .as_str()
                .and_then(|v| v.parse::<Uuid>().ok())
                .ok_or_else(internal)
        };
        let id = parse("id")?;
        let buyer = parse("buyer_id")?;
        let seller = parse("seller_id")?;
        let counterparty = if organization == buyer { seller } else { buyer };
        let recipients: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
                .bind(counterparty)
                .fetch_all(&mut *self.conn)
                .await?;
        for recipient in recipients {
            sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) VALUES($1,$2,'ORDER_UPDATE','New Trade Request',$3,$4,false,$5)").bind(Uuid::new_v4()).bind(recipient).bind(format!("A new trade request for {} MT at ${}/MT has been placed.",input.quantity_mt,trade["price_per_mt_usd"].as_str().unwrap_or_default())).bind(json!({"trade_id":id})).bind(at).execute(&mut *self.conn).await?;
        }
        let public_before = before.orders.iter().any(|o| o.public.is_some());
        let after = load_orders(self.conn, policy, config, &[input.order_id], at).await?;
        let public = public_before || after.iter().any(|o| o.public.is_some());
        let mut set = before.stage();
        set.push(MutationEffect{kind:MutationKind::TradeCreated,actor_id:actor,organization_id:organization,resource_type:"trade".into(),resource_id:id,
            changes:json!({"order_id":input.order_id,"quantity_mt":input.quantity_mt.to_string(),"price_per_mt_usd":trade["price_per_mt_usd"],"fame_terms_snapshot":trade["fame_terms_snapshot"],"certification_declared":input.certification_declared,"msds_available":input.msds_available}),
            event:Some(ParticipantEvent{event_type:"trade_created".into(),aggregate_type:"trade".into(),aggregate_id:id.to_string(),participant_org_ids:vec![buyer,seller],public_invalidated:public,
              fields:super::outbox::ParticipantFields::TradeCreated{product_id:trade["product_id"].as_str().and_then(|v|v.parse().ok()),activity:super::outbox::MatchActivity{source_kind:"CONFIRMED_TRADE".into(),scope:if trade["delivery_point_id"].is_null(){"UNKNOWN"}else{"DELIVERY_POINT"}.into(),demo_status:"REAL_ONLY".into(),unknown_count:0,market_product:trade["market_product"].as_str().map(str::to_owned),delivery_point_id:trade["delivery_point_id"].as_str().and_then(|v|v.parse().ok()),availability_window:trade["availability_window"].as_str().unwrap_or_default().into(),observed_at:at},id,status:"PENDING_CONFIRMATION".into(),quantity:input.quantity_mt.to_string(),price:trade["price_per_mt_usd"].as_str().unwrap_or_default().into(),product_name:trade["product_name"].as_str().unwrap_or_default().into(),fuel_type:trade["fuel_type"].as_str().unwrap_or_default().into(),region:trade["region"].as_str().unwrap_or_default().into()}}),at,audit_context:audit.clone()})?;
        self.finish(policy, config, set).await
    }

    /// Add already locked resting rows before applying a prepared match. Slice
    /// aggregates remain the pre-insert baseline captured by order entry.
    pub(super) async fn include_existing(
        &mut self,
        before: &mut BeforeEffects,
        policy: &MarketPolicy,
        config: &Config,
        ids: &[Uuid],
    ) -> Result<(), MarketWriteError> {
        if ids.iter().any(|id| !self.order_ids.contains(id)) {
            return Err(inconsistent());
        }
        let rows = load_orders(self.conn, policy, config, ids, before.at).await?;
        let targets = sqlx::query("SELECT t.id,t.watchlist_id,w.user_id,t.target_type,t.order_id,t.market_product_code,t.delivery_point_id,t.availability_window_code FROM watchlist_targets t JOIN watchlists w ON w.id=t.watchlist_id WHERE t.target_type='PIN' AND t.order_id=ANY($1) ORDER BY t.id")
            .bind(ids).fetch_all(&mut *self.conn).await?;
        for row in targets {
            before.targets.push(json!({"target_id":row.try_get::<Uuid,_>("id")?,"watchlist_id":row.try_get::<Uuid,_>("watchlist_id")?,"observer_id":row.try_get::<Uuid,_>("user_id")?,"target_type":row.try_get::<String,_>("target_type")?,"order_id":row.try_get::<Option<Uuid>,_>("order_id")?,"market_product":row.try_get::<String,_>("market_product_code")?,"delivery_point_id":row.try_get::<Uuid,_>("delivery_point_id")?,"availability_window":row.try_get::<String,_>("availability_window_code")?}));
        }
        before.orders.extend(rows);
        Ok(())
    }

    /// Stage match effects alongside route effects; the caller finalizes once.
    pub async fn matched(
        &mut self,
        set: &mut EffectSet,
        outcome: &super::matching::MatchOutcome,
        aggressor: &super::matching::OrderRow,
        actor: Uuid,
        audit_context: AuditContext,
    ) -> Result<(), MarketWriteError> {
        use super::outbox::{MatchActivity, ParticipantFields};
        if outcome.trades.is_empty() {
            return Ok(());
        }
        let mut participants = BTreeSet::from([aggressor.organization_id]);
        for trade in &outcome.trades {
            let row=sqlx::query("SELECT product_id,product_name,fuel_type,market_product,delivery_point_id,availability_window,buyer_provenance::text AS buyer_provenance,is_anonymous FROM trades WHERE id=$1")
                .bind(trade.id).fetch_one(&mut *self.conn).await?;
            let demo = row.get::<String, _>("buyer_provenance") == "DEMO";
            let point: Option<Uuid> = row.get("delivery_point_id");
            let activity = MatchActivity {
                source_kind: if demo { "DEMO_SEED" } else { "CONFIRMED_TRADE" }.into(),
                scope: if point.is_some() {
                    "DELIVERY_POINT"
                } else {
                    "UNKNOWN"
                }
                .into(),
                demo_status: if demo { "DEMO_ONLY" } else { "REAL_ONLY" }.into(),
                unknown_count: 0,
                market_product: row.get("market_product"),
                delivery_point_id: point,
                availability_window: row.get("availability_window"),
                observed_at: trade.confirmed_at,
            };
            participants.extend([trade.buyer_id, trade.seller_id]);
            set.confirmed_notifications
                .push((trade.id, trade.buyer_id, trade.seller_id));
            set.push(MutationEffect {
                kind:MutationKind::TradeAutoMatched,actor_id:actor,organization_id:aggressor.organization_id,
                resource_type:"trade".into(),resource_id:trade.id,
                changes:json!({"trade_id":trade.id,"bid_order_id":trade.bid_order_id,"ask_order_id":trade.ask_order_id,
                    "quantity_mt":trade.quantity_mt.to_string(),"price_per_mt_usd":trade.price_per_mt_usd.to_string(),
                    "buyer_org_id":trade.buyer_id,"seller_org_id":trade.seller_id}),
                event:Some(ParticipantEvent {event_type:"trade_auto_matched".into(),aggregate_type:"trade".into(),aggregate_id:trade.id.to_string(),
                    participant_org_ids:vec![trade.buyer_id,trade.seller_id],public_invalidated:true,
                    fields:ParticipantFields::TradeAutoMatched {activity,trade_id:trade.id,product_id:row.get("product_id"),
                        product_name:row.get::<Option<String>,_>("product_name").unwrap_or_default(),fuel_type:row.get::<Option<String>,_>("fuel_type").unwrap_or_default(),
                        quantity:trade.quantity_mt.to_string(),price:trade.price_per_mt_usd.to_string(),is_anonymous:row.get("is_anonymous")}}),
                at:set.before.at,audit_context:audit_context.clone(),
            })?;
        }
        let source =
            sqlx::query("SELECT t.market_product,t.delivery_point_name FROM trades t WHERE id=$1")
                .bind(outcome.trades[0].id)
                .fetch_one(&mut *self.conn)
                .await?;
        let demo = aggressor.provenance == "DEMO";
        set.push(MutationEffect {
            kind: MutationKind::OrdersMatched,
            actor_id: actor,
            organization_id: aggressor.organization_id,
            resource_type: "order".into(),
            resource_id: aggressor.id,
            changes: Value::Null,
            event: Some(ParticipantEvent {
                event_type: "orders_matched".into(),
                aggregate_type: "order".into(),
                aggregate_id: aggressor.id.to_string(),
                participant_org_ids: participants.into_iter().collect(),
                public_invalidated: true,
                fields: ParticipantFields::OrdersMatched {
                    activity: MatchActivity {
                        source_kind: if demo { "DEMO_SEED" } else { "LIVE_ORDER" }.into(),
                        scope: if aggressor.delivery_point_id.is_some() {
                            "DELIVERY_POINT"
                        } else {
                            "UNKNOWN"
                        }
                        .into(),
                        demo_status: if demo { "DEMO_ONLY" } else { "REAL_ONLY" }.into(),
                        unknown_count: 0,
                        market_product: source.get("market_product"),
                        delivery_point_id: aggressor.delivery_point_id,
                        availability_window: aggressor.availability_window.clone(),
                        observed_at: set.before.at,
                    },
                    order_id: aggressor.id,
                    matches: outcome.trades.len(),
                    is_demo_listing: demo,
                    delivery_point: source.get("delivery_point_name"),
                },
            }),
            at: set.before.at,
            audit_context,
        })?;
        Ok(())
    }

    pub async fn order_cancelled(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        if !matches!(effect.kind, MutationKind::OrderCancelled) {
            return Err(inconsistent());
        }
        self.apply(policy, config, before, effect).await
    }

    pub async fn inventory_changed(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        if !matches!(
            effect.kind,
            MutationKind::InventoryCreated
                | MutationKind::InventoryUpdated
                | MutationKind::InventoryDeleted
                | MutationKind::InventoryPublished
        ) {
            return Err(inconsistent());
        }
        self.apply(policy, config, before, effect).await
    }

    pub async fn support_changed(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        if !matches!(
            effect.kind,
            MutationKind::SupportContextStarted
                | MutationKind::SupportContextExited
                | MutationKind::SupportContextRevoked
                | MutationKind::SupportAuthorizationCreated
                | MutationKind::SupportAuthorizationRevoked
                | MutationKind::SupportCapabilityGranted
                | MutationKind::SupportCapabilityRevoked
        ) {
            return Err(inconsistent());
        }
        self.apply(policy, config, before, effect).await
    }

    pub async fn commission_changed(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        if !matches!(effect.kind, MutationKind::CommissionChanged) {
            return Err(inconsistent());
        }
        self.apply(policy, config, before, effect).await
    }

    /// The expiry facts bind the version transition; visibility was captured
    /// before the stock primitive changed these already locked orders.
    pub async fn orders_expired(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        facts: &[ExpiryEffect],
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        let mut set = before.stage();
        set.expired(facts.to_vec(), vec![effect])?;
        self.finish(policy, config, set).await
    }

    async fn apply(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        let mut set = before.stage();
        set.push(effect)?;
        self.finish(policy, config, set).await
    }

    /// Flush the complete transaction effect set, then close its economic phase once.
    pub async fn finish(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        set: EffectSet,
    ) -> Result<Uuid, MarketWriteError> {
        self.locked.require_economic_phase()?;
        if set.effects.is_empty() {
            return Err(inconsistent());
        }
        let before = set.before;
        let mut ids: Vec<_> = before
            .orders
            .iter()
            .map(|o| o.id)
            .chain(before.new_orders.iter().map(|n| n.order_id))
            .collect();
        ids.sort_unstable();
        let after = load_orders(self.conn, policy, config, &ids, before.at).await?;
        if before
            .orders
            .iter()
            .any(|a| !after.iter().any(|b| a.same_market_identity(b)))
        {
            return Err(inconsistent());
        }
        for new in &before.new_orders {
            let order = after
                .iter()
                .find(|o| o.id == new.order_id)
                .ok_or_else(inconsistent)?;
            if order.organization_id != new.organization_id
                || order.product_id != new.slice.product_id
                || order.point != Some(new.slice.delivery_point_id)
                || order.window != new.slice.availability_window
            {
                return Err(inconsistent());
            }
        }
        for fact in &set.expiry {
            if !after.iter().any(|o| {
                o.id == fact.order_id && o.version == fact.version && o.status == "EXPIRED"
            }) {
                return Err(inconsistent());
            }
        }
        if !ids.is_empty() && !set.effects.iter().any(|e| e.kind.changes_order_economics()) {
            return Err(inconsistent());
        }
        let aggregate_after =
            aggregates(self.conn, policy, config, &before.slices, before.at).await?;
        // Decline changes only its linked side. Keep both aggregate snapshots
        // for conditional slice evidence, but do not rewrite the opposite mirror.
        if set
            .effects
            .iter()
            .any(|e| matches!(e.kind, MutationKind::TradeDeclined))
        {
            let changed: Vec<_> = aggregate_after
                .iter()
                .filter(|aggregate| {
                    before.orders.iter().any(|order| {
                        aggregate["side"] == order.side
                            && aggregate["market_product"].as_str() == order.product.as_deref()
                            && aggregate["delivery_point_id"]
                                .as_str()
                                .and_then(|v| v.parse::<Uuid>().ok())
                                == order.point
                            && aggregate["availability_window"] == order.window
                    })
                })
                .cloned()
                .collect();
            rebuild_benchmarks(self.conn, &changed, before.at).await?;
        } else {
            rebuild_benchmarks(self.conn, &aggregate_after, before.at).await?;
        }
        let orders:Vec<_>=after.iter().enumerate().map(|(index,b)|json!({"order_id":b.id,"transition_index":index,"before":before.orders.iter().find(|a|a.id==b.id).map(OrderState::evidence),"after":b.evidence()})).collect();
        for effect in &set.effects {
            if let Some(action) = effect.kind.audit_action() {
                let actorless = matches!(effect.kind, MutationKind::OrderExpired);
                append_audit(
                    self.conn,
                    &AuditFact {
                        id: Uuid::new_v4(),
                        actor_id: if actorless {
                            None
                        } else {
                            Some(effect.actor_id)
                        },
                        action: action.into(),
                        resource_type: effect.resource_type.clone(),
                        resource_id: Some(effect.resource_id.to_string()),
                        changes: Some(effect.changes.clone()),
                        ip_address: if actorless {
                            None
                        } else {
                            effect.audit_context.ip_address.clone()
                        },
                        request_id: if actorless {
                            None
                        } else {
                            effect.audit_context.request_id.clone()
                        },
                        at: effect.at,
                    },
                )
                .await?;
            }
        }
        for (trade, buyer, seller) in &set.confirmed_notifications {
            let recipients: Vec<Uuid> = sqlx::query_scalar(
                "SELECT id FROM users WHERE organization_id=ANY($1::uuid[]) ORDER BY id",
            )
            .bind([*buyer, *seller].as_slice())
            .fetch_all(&mut *self.conn)
            .await?;
            for recipient in recipients {
                sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) VALUES($1,$2,'TRADE_CONFIRMED','Auto-Matched Trade','Your order was automatically matched',$3,false,$4)")
                    .bind(Uuid::new_v4()).bind(recipient).bind(json!({"trade_id":trade,"auto_matched":true})).bind(before.at).execute(&mut *self.conn).await?;
            }
        }
        for effect in &set.effects {
            if matches!(effect.kind, MutationKind::OrderCreated)
                && effect.changes["creation_method"] == "MARKET_SUPPORT"
            {
                let recipients: Vec<Uuid> =
                    sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
                        .bind(effect.organization_id)
                        .fetch_all(&mut *self.conn)
                        .await?;
                for recipient in recipients {
                    sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) VALUES($1,$2,'ORDER_UPDATE','Listing set up by Verdaxis Support','Verdaxis Support published an authorized listing for your organization.',$3,false,$4)")
                        .bind(Uuid::new_v4()).bind(recipient).bind(json!({"order_id":effect.resource_id,"creation_method":"MARKET_SUPPORT"})).bind(before.at).execute(&mut *self.conn).await?;
                }
            }
        }
        self.locked.begin_target_phase()?;
        sync_pin_display(self.conn, &after).await?;
        let mutation_id = Uuid::new_v4();
        for effect in set.effects {
            if let Some(event) = effect.event {
                let evidence = ProducerEvidence {
                    mutation_id,
                    captured_at: before.at,
                    orders: orders.clone(),
                    aggregates_before: before.aggregates.clone(),
                    aggregates_after: aggregate_after.clone(),
                    targets: before.targets.clone(),
                };
                super::outbox::append(self.conn, &event, &evidence).await?;
            }
        }
        Ok(mutation_id)
    }
}

pub(crate) async fn order_visible(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    config: &Config,
    id: Uuid,
    at: DateTime<Utc>,
) -> Result<bool, MarketWriteError> {
    let sql = order_query_sql("SELECT id FROM eligible_orders WHERE id=$14");
    let clock = VisibilityClock::at(at);
    Ok(
        bind_order_query(&sql, policy, &clock, config, &PublicOrderFilters::default())?
            .bind(id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

fn inconsistent() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::InventoryInconsistent)
}
fn internal() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn slices(orders: &[OrderState]) -> BTreeSet<Slice> {
    orders.iter().filter_map(OrderState::slice).collect()
}
fn slice_values(slices: &BTreeSet<Slice>) -> Value {
    json!(
        slices
            .iter()
            .map(|s| json!({"product":s.product,"point":s.point,"window_code":s.window}))
            .collect::<Vec<_>>()
    )
}

async fn load_orders(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    config: &Config,
    ids: &[Uuid],
    at: DateTime<Utc>,
) -> Result<Vec<OrderState>, MarketWriteError> {
    let clock = VisibilityClock::at(at);
    let sql = order_query_sql(
        "SELECT e.*, /* private_digest_input(base) */, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label FROM eligible_orders e JOIN orderbook_orders base ON base.id=e.id JOIN organizations org ON org.id=e.organization_id WHERE e.id=ANY($14)",
    );
    let rows = bind_order_query(&sql, policy, &clock, config, &PublicOrderFilters::default())?
        .bind(ids)
        .fetch_all(&mut *conn)
        .await?;
    let mut public = BTreeMap::new();
    for row in rows {
        let projection = PublicOrder::from_row(&row).map_err(|_| internal())?;
        let qualified = candidate_is_benchmark_qualified(policy, &projection.private, &clock);
        public.insert(
            row.try_get::<Uuid, _>("id")?,
            (
                projection.public(false, None).map_err(|_| internal())?,
                qualified,
            ),
        );
    }
    let rows=sqlx::query("SELECT o.id,o.organization_id,o.product_id,o.delivery_point_id,o.availability_window,o.version,o.status::text AS status,o.side::text AS side,o.price_per_mt_usd,o.quantity_mt,o.remaining_quantity_mt,o.provenance::text AS provenance,dp.name AS point_name FROM orderbook_orders o LEFT JOIN delivery_points dp ON dp.id=o.delivery_point_id WHERE o.id=ANY($1) ORDER BY o.id")
        .bind(ids).fetch_all(conn).await?;
    if rows.len() != ids.len() {
        return Err(inconsistent());
    }
    rows.iter()
        .map(|r| {
            let id: Uuid = r.try_get("id")?;
            let product_id: Uuid = r.try_get("product_id")?;
            let product = policy
                .products
                .iter()
                .find(|p| p.id == product_id)
                .map(|p| p.code.clone());
            let window: String = r.try_get("availability_window")?;
            let admitted = public.remove(&id);
            Ok(OrderState {
                id,
                organization_id: r.try_get("organization_id")?,
                product_id,
                product,
                point: r.try_get("delivery_point_id")?,
                window: normalize_window(&window, at.date_naive()).map_err(|_| inconsistent())?,
                version: r.try_get("version")?,
                status: r.try_get("status")?,
                side: r.try_get("side")?,
                price: r.try_get("price_per_mt_usd")?,
                quantity: r.try_get("quantity_mt")?,
                remaining: r.try_get("remaining_quantity_mt")?,
                point_name: r.try_get("point_name")?,
                qualified: admitted.as_ref().is_some_and(|(_, q)| *q),
                public: admitted.map(|(p, _)| p),
                provenance: r.try_get("provenance")?,
            })
        })
        .collect()
}

async fn aggregates(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    config: &Config,
    slices: &BTreeSet<Slice>,
    at: DateTime<Utc>,
) -> Result<Vec<Value>, MarketWriteError> {
    let mut values = Vec::new();
    let clock = VisibilityClock::at(at);
    for slice in slices {
        for side in ["BID", "ASK"] {
            let sql = order_query_sql(
                "SELECT e.remaining_quantity_mt,e.price_per_mt_usd,e.provenance::text AS provenance, /* private_digest_input(e) */ FROM eligible_orders e",
            );
            let filters = PublicOrderFilters {
                surface: OrderSurface::Candidates,
                product_id: Some(slice.product_id),
                delivery_point_id: Some(slice.point),
                availability_window: Some(&slice.window),
                side: Some(side),
                ..Default::default()
            };
            let rows = bind_order_query(&sql, policy, &clock, config, &filters)?
                .fetch_all(&mut *conn)
                .await?;
            let mut real = Vec::new();
            let mut demo = Vec::new();
            for row in rows {
                let private: Value = row.try_get("private_digest_input")?;
                if !candidate_is_benchmark_qualified(policy, &private, &clock) {
                    continue;
                }
                let economic = (
                    row.try_get::<Decimal, _>("remaining_quantity_mt")?,
                    row.try_get::<Decimal, _>("price_per_mt_usd")?,
                );
                match row.try_get::<String, _>("provenance")?.as_str() {
                    "REAL" => real.push(economic),
                    "DEMO" => demo.push(economic),
                    _ => return Err(inconsistent()),
                }
            }
            let economic: Vec<_> = real.iter().chain(&demo).copied().collect();
            let quantity = total_quantity(&economic)?;
            let best = best_price(&economic, side);
            let vwap = qualified_vwap(&economic);
            values.push(
                json!({"market_product":slice.product,"delivery_point_id":slice.point,
                "availability_window":slice.window,"side":side,"quantity_mt":quantity.to_string(),
                "order_count":economic.len(),"real_count":real.len(),"demo_count":demo.len(),
                "real":provenance_aggregate(&real,side)?,"demo":provenance_aggregate(&demo,side)?,
                "best_price_per_mt_usd":best.map(|p|p.to_string()),
                "benchmark_price_per_mt_usd":vwap.map(|p|p.to_string())}),
            );
        }
    }
    Ok(values)
}

fn total_quantity(rows: &[(Decimal, Decimal)]) -> Result<Decimal, MarketWriteError> {
    rows.iter().try_fold(Decimal::ZERO, |sum, (q, _)| {
        sum.checked_add(*q).ok_or_else(inconsistent)
    })
}
fn best_price(rows: &[(Decimal, Decimal)], side: &str) -> Option<Decimal> {
    rows.iter()
        .map(|(_, price)| *price)
        .reduce(|a, b| if side == "BID" { a.max(b) } else { a.min(b) })
}
fn provenance_aggregate(
    rows: &[(Decimal, Decimal)],
    side: &str,
) -> Result<Value, MarketWriteError> {
    Ok(
        json!({"quantity_mt":total_quantity(rows)?.to_string(),"order_count":rows.len(),
        "best_price_per_mt_usd":best_price(rows,side).map(|p|p.to_string())}),
    )
}

async fn rebuild_benchmarks(
    conn: &mut PgConnection,
    values: &[Value],
    at: DateTime<Utc>,
) -> Result<(), MarketWriteError> {
    for value in values {
        let text = |name| value[name].as_str().ok_or_else(inconsistent);
        let point: Uuid = text("delivery_point_id")?
            .parse()
            .map_err(|_| inconsistent())?;
        sqlx::query("DELETE FROM live_slice_benchmarks WHERE side=$1 AND market_product=$2 AND delivery_point_id=$3 AND availability_window=$4")
            .bind(text("side")?).bind(text("market_product")?).bind(point).bind(text("availability_window")?).execute(&mut *conn).await?;
        if let Some(price) = value["benchmark_price_per_mt_usd"].as_str() {
            sqlx::query("INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'live_slice_vwap',$9,$9)")
                .bind(Uuid::new_v4()).bind(text("side")?).bind(text("market_product")?).bind(point).bind(text("availability_window")?).bind(price.parse::<Decimal>().map_err(|_|inconsistent())?).bind(text("quantity_mt")?.parse::<Decimal>().map_err(|_|inconsistent())?).bind(i32::try_from(value["order_count"].as_u64().ok_or_else(inconsistent)?).map_err(|_|inconsistent())?).bind(at).execute(&mut *conn).await?;
        }
    }
    Ok(())
}

async fn sync_pin_display(
    conn: &mut PgConnection,
    orders: &[OrderState],
) -> Result<(), MarketWriteError> {
    let ids: Vec<_> = orders.iter().map(|o| o.id).collect();
    let targets=sqlx::query("SELECT id,order_id FROM watchlist_targets WHERE target_type='PIN' AND order_id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&ids).fetch_all(&mut *conn).await?;
    for target in targets {
        let id: Uuid = target.try_get("id")?;
        let order_id: Uuid = target.try_get("order_id")?;
        let order = orders
            .iter()
            .find(|o| o.id == order_id)
            .ok_or_else(inconsistent)?;
        // A PIN keeps its market codes when the order has no policy product or point.
        sqlx::query("UPDATE watchlist_targets SET snapshot_price_per_mt_usd=$3::numeric::double precision,snapshot_quantity_mt=$4::numeric::double precision,snapshot_remaining_quantity_mt=$5::numeric::double precision,snapshot_status=$6,snapshot_side=$7,snapshot_market_product=$8,snapshot_delivery_point_name=$9,snapshot_availability_window=$10,snapshot_counterparty_label=NULL,market_product_code=COALESCE($8,market_product_code),delivery_point_id=COALESCE($11,delivery_point_id),availability_window_code=$10 WHERE id=$1 AND target_type='PIN' AND order_id=$2")
            .bind(id).bind(order.id).bind(order.price).bind(order.quantity).bind(order.remaining).bind(&order.status).bind(&order.side).bind(&order.product).bind(&order.point_name).bind(&order.window).bind(order.point).execute(&mut *conn).await?;
    }
    Ok(())
}

#[cfg(feature = "contract-tests")]
pub mod contract {
    use super::super::authority::admission::AdmissionClass;
    use super::super::locks::{
        LockClass, MarketOperation, OperationLockPlan, PartyLockPreview, SliceKey,
    };
    use super::super::outbox::ParticipantFields;
    use super::super::transaction::{
        BoxFuture, CommittedCommand, LockedMarketTransaction, MarketAttemptError, MarketCommand,
        MarketPreview, run_market_command,
    };
    use super::super::{AuthenticatedActor, Operation, ReceiptRecord};
    use super::*;
    use crate::auth::VerifiedAccess;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    #[derive(Clone)]
    pub struct EffectProbe {
        pub order_id: Uuid,
        pub actor_id: Uuid,
        pub organization_id: Uuid,
        pub kind: MutationKind,
        pub replay: bool,
        pub commit: bool,
    }
    pub struct ProbeResult {
        pub visible_from_writer: i64,
        pub visible_from_observer: i64,
        pub writer_pin_status: String,
        pub observer_pin_status: String,
    }
    fn policy() -> Result<MarketPolicy, MarketWriteError> {
        serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json"))
            .map_err(|_| internal())
    }
    fn config() -> Result<Config, MarketWriteError> {
        serde_json::from_value(json!({"environment":"test","release_sha":"test",
            "migration_revision":"p89_20261022_watchlist_producer","database_url":"",
            "db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,
            "python_budget":0,"configured_max_connections":100}))
        .map_err(|_| internal())
    }
    fn access(actor: Uuid) -> VerifiedAccess {
        VerifiedAccess {
            subject: actor,
            issued_at: Some(Utc::now()),
            issuance_out_of_range: false,
        }
    }
    async fn plan(
        conn: &mut PgConnection,
        probe: &EffectProbe,
        key: Option<&str>,
    ) -> Result<MarketPreview, MarketAttemptError> {
        let row=sqlx::query("SELECT organization_id,owner_user_id,product_id,delivery_point_id,availability_window,inventory_item_id FROM orderbook_orders WHERE id=$1")
            .bind(probe.order_id).fetch_one(conn).await?;
        let organization: Uuid = row.try_get("organization_id")?;
        let owner: Option<Uuid> = row.try_get("owner_user_id")?;
        let product: Uuid = row.try_get("product_id")?;
        let point: Option<Uuid> = row.try_get("delivery_point_id")?;
        let window: String = row.try_get("availability_window")?;
        let item: Option<Uuid> = row.try_get("inventory_item_id")?;
        let parties = PartyLockPreview::new(
            [organization],
            owner,
            probe.actor_id,
            json!({"order_id":probe.order_id,"organization_id":organization,"owner_user_id":owner,
                "product_id":product,"delivery_point_id":point,"availability_window":window,"inventory_item_id":item}),
        );
        let mut locks = OperationLockPlan::new(MarketOperation::Cancel);
        locks.parties(&parties)?;
        if let Some(key) = key {
            locks.idempotency(probe.actor_id, Operation::OrderCancel.as_str(), key)?;
        }
        locks.slices([SliceKey {
            product_id: product,
            delivery_point_id: point,
            availability_window: window,
        }])?;
        locks.orders([probe.order_id])?;
        locks.inventory(item)?;
        Ok(MarketPreview { parties, locks })
    }
    fn resource_type(kind: MutationKind) -> &'static str {
        match kind {
            MutationKind::OrderCancelled | MutationKind::OrderExpired => "order",
            MutationKind::InventoryCreated
            | MutationKind::InventoryUpdated
            | MutationKind::InventoryDeleted
            | MutationKind::InventoryPublished => "inventory",
            MutationKind::SupportCapabilityGranted | MutationKind::SupportCapabilityRevoked => {
                "staff_capability_assignment"
            }
            MutationKind::SupportAuthorizationCreated
            | MutationKind::SupportAuthorizationRevoked => "market_support_authorization",
            MutationKind::CommissionChanged => "commission",
            _ => "market_support_context",
        }
    }
    pub(in crate::order_write) async fn effect(
        conn: &mut PgConnection,
        probe: &EffectProbe,
        policy: &MarketPolicy,
        at: DateTime<Utc>,
    ) -> Result<MutationEffect, MarketWriteError> {
        let id = probe.order_id;
        let changes = match probe.kind {
            MutationKind::OrderCancelled => {
                json!({"status":"CANCELLED","reason":null,"actor_user_id":probe.actor_id,
                "effective_organization_id":probe.organization_id,"original_principal_user_id":probe.actor_id,
                "cancellation_principal_user_id":probe.actor_id,"support_context_id":null,"original_support_context_id":null,"version":2})
            }
            MutationKind::OrderExpired => json!({"status":"EXPIRED","released_quantity_mt":"10"}),
            MutationKind::InventoryPublished => {
                json!({"inventory_item_id":id,"listing_id":id,"quantity_mt":"10","price_per_mt_usd":"100","status":"OPEN"})
            }
            MutationKind::SupportCapabilityGranted | MutationKind::SupportCapabilityRevoked => {
                json!({"target_user_id":probe.actor_id,"capability":"MARKET_SUPPORT_LISTINGS","reason":"Fixture request"})
            }
            MutationKind::SupportContextStarted => {
                json!({"organization_id":probe.organization_id,"support_reference":"Fixture request","scope":"ASSISTED_ORDER_ENTRY","expires_at":"2026-12-01T00:00:00Z"})
            }
            MutationKind::SupportContextExited => json!({"status":"EXITED"}),
            MutationKind::SupportContextRevoked => {
                json!({"status":"REVOKED","reason":"Fixture request","capability_assignment_id":id})
            }
            MutationKind::CommissionChanged => json!({"status":{"from":"PENDING","to":"PAID"}}),
            _ => Value::Null,
        };
        let event = match probe.kind {
            MutationKind::OrderCancelled | MutationKind::InventoryPublished => {
                let row=sqlx::query("SELECT o.side::text AS side,o.product_id,o.delivery_point_id,o.availability_window,o.updated_at,o.price_per_mt_usd,o.remaining_quantity_mt,p.name AS product_name,p.fuel_type,dp.name AS delivery_point,dp.region FROM orderbook_orders o JOIN products p ON p.id=o.product_id JOIN delivery_points dp ON dp.id=o.delivery_point_id WHERE o.id=$1")
                    .bind(id).fetch_one(&mut *conn).await?;
                let product_id: Uuid = row.try_get("product_id")?;
                let product = policy
                    .products
                    .iter()
                    .find(|p| p.id == product_id)
                    .ok_or_else(inconsistent)?;
                let activity = super::super::outbox::OrderActivity {
                    id,
                    side: row.try_get("side")?,
                    product_name: row.try_get("product_name")?,
                    fuel_type: row.try_get("fuel_type")?,
                    region: row.try_get("region")?,
                    source_kind: "LIVE_ORDER".into(),
                    scope: "DELIVERY_POINT".into(),
                    demo_status: "REAL_ONLY".into(),
                    is_demo_listing: false,
                    unknown_count: 0,
                    market_product: Some(product.code.clone()),
                    delivery_point_id: row.try_get("delivery_point_id")?,
                    availability_window: row.try_get("availability_window")?,
                    observed_at: Some(row.try_get("updated_at")?),
                    delivery_point: row.try_get("delivery_point")?,
                };
                let created = matches!(probe.kind, MutationKind::InventoryPublished);
                let fields = if created {
                    ParticipantFields::OrderCreated {
                        activity,
                        price: row.try_get::<Decimal, _>("price_per_mt_usd")?.to_string(),
                        quantity: row
                            .try_get::<Decimal, _>("remaining_quantity_mt")?
                            .to_string(),
                    }
                } else {
                    ParticipantFields::OrderCancelled(activity)
                };
                Some(ParticipantEvent {
                    event_type: if created {
                        "order_created"
                    } else {
                        "order_cancelled"
                    }
                    .into(),
                    aggregate_type: "order".into(),
                    aggregate_id: id.to_string(),
                    participant_org_ids: vec![probe.organization_id],
                    fields,
                    public_invalidated: true,
                })
            }
            MutationKind::OrderExpired => Some(ParticipantEvent {
                event_type: "order_expired".into(),
                aggregate_type: "order".into(),
                aggregate_id: id.to_string(),
                participant_org_ids: vec![probe.organization_id],
                fields: ParticipantFields::OrderExpired { id },
                public_invalidated: false,
            }),
            MutationKind::InventoryCreated
            | MutationKind::InventoryUpdated
            | MutationKind::InventoryDeleted => Some(ParticipantEvent {
                event_type: match probe.kind {
                    MutationKind::InventoryCreated => "inventory_created",
                    MutationKind::InventoryUpdated => "inventory_updated",
                    _ => "inventory_deleted",
                }
                .into(),
                aggregate_type: "inventory".into(),
                aggregate_id: id.to_string(),
                participant_org_ids: vec![probe.organization_id],
                fields: ParticipantFields::Inventory { id },
                public_invalidated: true,
            }),
            _ => None,
        };
        Ok(MutationEffect {
            kind: probe.kind,
            actor_id: probe.actor_id,
            organization_id: probe.organization_id,
            resource_type: resource_type(probe.kind).into(),
            resource_id: id,
            changes,
            event,
            at,
            audit_context: if matches!(probe.kind, MutationKind::CommissionChanged) {
                AuditContext::new(
                    Some("192.0.2.44".into()),
                    Some("11111111-2222-4333-8444-555555555555".into()),
                )?
            } else {
                AuditContext::default()
            },
        })
    }
    async fn apply_port(
        port: &mut EffectPort<'_>,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        match effect.kind {
            MutationKind::OrderCancelled => {
                port.order_cancelled(policy, config, before, effect).await
            }
            MutationKind::OrderExpired => Err(inconsistent()),
            MutationKind::InventoryCreated
            | MutationKind::InventoryUpdated
            | MutationKind::InventoryDeleted
            | MutationKind::InventoryPublished => {
                port.inventory_changed(policy, config, before, effect).await
            }
            MutationKind::CommissionChanged => {
                port.commission_changed(policy, config, before, effect)
                    .await
            }
            _ => port.support_changed(policy, config, before, effect).await,
        }
    }
    async fn perform(
        tx: &mut LockedMarketTransaction<'_>,
        probe: &EffectProbe,
        at: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        let policy = policy()?;
        let config = config()?;
        let ids = if matches!(
            probe.kind,
            MutationKind::OrderCancelled | MutationKind::InventoryPublished
        ) {
            vec![probe.order_id]
        } else {
            Vec::new()
        };
        let before = tx
            .effects()?
            .capture_before(&policy, &config, &ids, at)
            .await?;
        if matches!(probe.kind, MutationKind::OrderCancelled) {
            tx.effect_stage_order(probe.order_id, "CANCELLED").await?;
        }
        let descriptor = tx.effect_descriptor(probe, &policy, at).await?;
        apply_port(&mut tx.effects()?, &policy, &config, before, descriptor).await?;
        Ok(())
    }
    pub async fn run_effect_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
    ) -> Result<ProbeResult, MarketWriteError> {
        if probe.kind.is_retired() {
            return Err(MarketWriteError::new(
                MarketErrorCode::MarketSupportLegacyMutationRetired,
            ));
        }
        let mut tx = pool.begin().await?;
        let plan = plan(&mut tx, &probe, None).await.map_err(attempt_error)?;
        let locked = plan.locks.acquire(&mut tx).await?;
        let at = Utc::now();
        let mut port = LockedMarketTransaction::from_effect_fixture(&mut tx, &plan.locks, &locked);
        if !probe.replay {
            perform(&mut port, &probe, at).await?;
        }
        let visible_from_writer = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&mut *tx)
            .await?;
        let writer_pin_status =
            sqlx::query_scalar("SELECT snapshot_status FROM watchlist_targets WHERE order_id=$1")
                .bind(probe.order_id)
                .fetch_one(&mut *tx)
                .await?;
        let mut observer = pool.acquire().await?;
        let visible_from_observer = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&mut *observer)
            .await?;
        let observer_pin_status =
            sqlx::query_scalar("SELECT snapshot_status FROM watchlist_targets WHERE order_id=$1")
                .bind(probe.order_id)
                .fetch_one(&mut *observer)
                .await?;
        if probe.commit {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(ProbeResult {
            visible_from_writer,
            visible_from_observer,
            writer_pin_status,
            observer_pin_status,
        })
    }
    fn attempt_error(error: MarketAttemptError) -> MarketWriteError {
        match error {
            MarketAttemptError::Database(e) | MarketAttemptError::PublicationFlush(e) => e.into(),
            MarketAttemptError::RecoveryAbsent => {
                MarketWriteError::new(MarketErrorCode::IdempotencyConflict)
            }
            MarketAttemptError::Refusal(e) => e,
            MarketAttemptError::PreviewChanged => {
                MarketWriteError::new(MarketErrorCode::MarketSliceChanged)
            }
        }
    }
    fn cancel_request_hash() -> String {
        super::super::command_request_hash(&super::super::Canonical::Object(BTreeMap::from([(
            "operation".into(),
            super::super::Canonical::Text("order.cancel".into()),
        )])))
    }
    struct OrderedEffectCommand {
        probe: EffectProbe,
        key: String,
    }
    impl MarketCommand for OrderedEffectCommand {
        type Output = CommittedCommand;
        fn preview<'a>(
            &'a self,
            conn: &'a mut PgConnection,
            _actor: Uuid,
        ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
            Box::pin(async move { plan(conn, &self.probe, Some(&self.key)).await })
        }
        fn replay<'a>(
            &'a self,
            tx: &'a mut LockedMarketTransaction<'_>,
            actor: &'a AuthenticatedActor,
        ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
            Box::pin(async move {
                let context = tx
                    .execution_context(actor, self.probe.organization_id, Utc::now(), true)
                    .await?;
                Ok(tx
                    .replay_receipt(
                        &context,
                        Operation::OrderCancel,
                        self.probe.order_id,
                        &self.key,
                        &cancel_request_hash(),
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
                let context = tx
                    .execution_context(actor, self.probe.organization_id, Utc::now(), true)
                    .await?;
                perform(tx, &self.probe, context.now()).await?;
                let response = CommittedCommand::new(204, Value::Null, false);
                tx.record_receipt(
                    &context,
                    &ReceiptRecord {
                        id: Uuid::new_v4(),
                        actor_user_id: self.probe.actor_id,
                        effective_organization_id: self.probe.organization_id,
                        support_context_id: None,
                        operation: Operation::OrderCancel,
                        idempotency_key: self.key.clone(),
                        request_hash: cancel_request_hash(),
                        resource_id: self.probe.order_id,
                        response_status: 204,
                        response_body: Value::Null,
                    },
                )
                .await?;
                Ok(response)
            })
        }
        fn prepare_response(&self, value: &Self::Output) -> Result<(), MarketAttemptError> {
            serde_json::to_vec(&value.response_body).map_err(|_| internal())?;
            Ok(())
        }
    }
    pub async fn run_ordered_effect_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
        key: String,
    ) -> Result<CommittedCommand, MarketWriteError> {
        run_market_command(
            pool.clone(),
            Arc::new(Semaphore::new(1)),
            access(probe.actor_id),
            OrderedEffectCommand { probe, key },
            AdmissionClass::Cleanup,
        )
        .await
    }
    pub async fn run_expiry_effect_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
    ) -> Result<Vec<ExpiryEffect>, MarketWriteError> {
        let mut tx = pool.begin().await?;
        let preview = plan(&mut tx, &probe, None).await.map_err(attempt_error)?;
        let locked = preview.locks.acquire(&mut tx).await?;
        let row=sqlx::query("SELECT product_id,delivery_point_id,availability_window,side::text AS side FROM orderbook_orders WHERE id=$1")
            .bind(probe.order_id).fetch_one(&mut *tx).await?;
        let window: String = row.try_get("availability_window")?;
        let side: String = row.try_get("side")?;
        let slice = super::super::ExpirySlice {
            product_id: row.try_get("product_id")?,
            delivery_point_id: row.try_get("delivery_point_id")?,
            availability_window: &window,
            side: &side,
        };
        let policy = policy()?;
        let config = config()?;
        let at = Utc::now();
        let mut port =
            LockedMarketTransaction::from_effect_fixture(&mut tx, &preview.locks, &locked);
        let before = port
            .effects()?
            .capture_before(&policy, &config, &[probe.order_id], at)
            .await?;
        let facts = port.stock_ledger()?.expire_slice(&slice, at).await?;
        let effect = port.effect_descriptor(&probe, &policy, at).await?;
        port.effects()?
            .orders_expired(&policy, &config, before, &facts, effect)
            .await?;
        tx.commit().await?;
        Ok(facts)
    }
    pub async fn run_terminal_phase_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
    ) -> Result<Vec<String>, MarketWriteError> {
        let mut tx = pool.begin().await?;
        let preview = plan(&mut tx, &probe, None).await.map_err(attempt_error)?;
        let locked = preview.locks.acquire(&mut tx).await?;
        let mut port =
            LockedMarketTransaction::from_effect_fixture(&mut tx, &preview.locks, &locked);
        perform(&mut port, &probe, Utc::now()).await?;
        let mut errors = Vec::new();
        // Each plan uses a different economic class. The phase guard runs before SQL.
        for class in [
            LockClass::Order,
            LockClass::User,
            LockClass::Slice,
            LockClass::Inventory,
        ] {
            let mut next = OperationLockPlan::new(MarketOperation::Cancel);
            let parties = PartyLockPreview::new(
                [probe.organization_id],
                [probe.actor_id],
                probe.actor_id,
                json!({}),
            );
            next.parties(&parties)?;
            match class {
                LockClass::Order => {
                    next.orders([probe.order_id])?;
                }
                LockClass::Inventory => {
                    next.inventory([probe.order_id])?;
                }
                LockClass::Slice => {
                    next.slices([SliceKey {
                        product_id: probe.order_id,
                        delivery_point_id: None,
                        availability_window: "SPOT".into(),
                    }])?;
                }
                _ => {}
            }
            let error = port
                .effect_reacquire(&next)
                .await
                .expect_err("target phase must refuse later acquisition");
            errors.push(error.code.as_str().to_owned());
        }
        tx.rollback().await?;
        Ok(errors)
    }
    pub struct BatchProbeResult {
        pub staged_audits: i64,
        pub staged_events: i64,
        pub observer_audits: i64,
        pub observer_events: i64,
        pub late_lock_code: String,
    }
    async fn batch_result(
        pool: &sqlx::PgPool,
        conn: &mut PgConnection,
        locked: &Locked,
    ) -> Result<BatchProbeResult, MarketWriteError> {
        let staged_audits = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
            .fetch_one(&mut *conn)
            .await?;
        let staged_events = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&mut *conn)
            .await?;
        let mut observer = pool.acquire().await?;
        let observer_audits = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
            .fetch_one(&mut *observer)
            .await?;
        let observer_events = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&mut *observer)
            .await?;
        let late_lock_code = locked
            .require_economic_phase()
            .expect_err("terminal effect phase")
            .code
            .as_str()
            .to_owned();
        Ok(BatchProbeResult {
            staged_audits,
            staged_events,
            observer_audits,
            observer_events,
            late_lock_code,
        })
    }
    async fn slice_for(conn: &mut PgConnection, id: Uuid) -> Result<EffectSlice, MarketWriteError> {
        let row=sqlx::query("SELECT product_id,delivery_point_id,availability_window FROM orderbook_orders WHERE id=$1").bind(id).fetch_one(conn).await?;
        Ok(EffectSlice {
            product_id: row.try_get("product_id")?,
            delivery_point_id: row.try_get("delivery_point_id")?,
            availability_window: row.try_get("availability_window")?,
        })
    }
    pub async fn run_new_order_effect_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
        new_id: Uuid,
    ) -> Result<(), MarketWriteError> {
        let mut tx = pool.begin().await?;
        let preview = plan(&mut tx, &probe, None).await.map_err(attempt_error)?;
        let locked = preview.locks.acquire(&mut tx).await?;
        let slice = slice_for(&mut tx, probe.order_id).await?;
        let policy = policy()?;
        let config = config()?;
        let at = Utc::now();
        let before = EffectPort::new(&mut tx, &[], &locked)
            .capture_mutation(
                &policy,
                &config,
                &[],
                std::slice::from_ref(&slice),
                &[NewOrderEffect {
                    order_id: new_id,
                    organization_id: probe.organization_id,
                    slice: slice.clone(),
                }],
                at,
            )
            .await?;
        sqlx::query("SELECT effect_fixture_insert_order($1,$2)")
            .bind(probe.order_id)
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        let mut next = probe.clone();
        next.order_id = new_id;
        let descriptor = effect(&mut tx, &next, &policy, at).await?;
        let mut set = before.stage();
        set.push(descriptor)?;
        EffectPort::new(&mut tx, &[], &locked)
            .finish(&policy, &config, set)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn run_revoke_batch_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
        assignment: Uuid,
        contexts: Vec<Uuid>,
        commit: bool,
    ) -> Result<BatchProbeResult, MarketWriteError> {
        let mut tx = pool.begin().await?;
        let mut preview = plan(&mut tx, &probe, None).await.map_err(attempt_error)?;
        // Context rows have already been declared with all actors by the caller.
        let users: Vec<Uuid> = sqlx::query_scalar(
            "SELECT actor_user_id FROM market_support_contexts WHERE id=ANY($1)",
        )
        .bind(&contexts)
        .fetch_all(&mut *tx)
        .await?;
        let parties = PartyLockPreview::new(
            [probe.organization_id],
            users,
            probe.actor_id,
            json!({"contexts":contexts}),
        );
        let mut locks = OperationLockPlan::new(MarketOperation::Cancel);
        locks.parties(&parties)?;
        locks.resources(LockClass::Capability, [assignment])?;
        locks.resources(LockClass::Context, contexts.clone())?;
        preview.parties = parties;
        preview.locks = locks;
        let locked = preview.locks.acquire(&mut tx).await?;
        let policy = policy()?;
        let config = config()?;
        let at = Utc::now();
        let before = EffectPort::new(&mut tx, &[], &locked)
            .capture_before(&policy, &config, &[], at)
            .await?;
        sqlx::query("SELECT effect_fixture_revoke($1,$2,$3)")
            .bind(assignment)
            .bind(&contexts)
            .bind(probe.actor_id)
            .execute(&mut *tx)
            .await?;
        let mut set = before.stage();
        let mut descriptor = effect(&mut tx, &probe, &policy, at).await?;
        descriptor.resource_id = assignment;
        set.push(descriptor)?;
        for id in contexts {
            let mut context_probe = probe.clone();
            context_probe.kind = MutationKind::SupportContextRevoked;
            context_probe.order_id = id;
            let mut descriptor = effect(&mut tx, &context_probe, &policy, at).await?;
            descriptor.changes = json!({"status":"REVOKED","reason":"Fixture request","capability_assignment_id":assignment});
            set.push(descriptor)?;
        }
        EffectPort::new(&mut tx, &[], &locked)
            .finish(&policy, &config, set)
            .await?;
        let result = batch_result(pool, &mut tx, &locked).await?;
        if commit {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }
    pub async fn run_expiry_batch_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
        mut ids: Vec<Uuid>,
        commit: bool,
    ) -> Result<BatchProbeResult, MarketWriteError> {
        let mut tx = pool.begin().await?;
        ids.sort_unstable();
        let rows = sqlx::query(
            "SELECT organization_id,owner_user_id FROM orderbook_orders WHERE id=ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
        let orgs = rows
            .iter()
            .map(|r| r.get::<Uuid, _>("organization_id"))
            .collect::<Vec<_>>();
        let users = rows
            .iter()
            .filter_map(|r| r.get::<Option<Uuid>, _>("owner_user_id"))
            .collect::<Vec<_>>();
        let slice = slice_for(&mut tx, probe.order_id).await?;
        let parties = PartyLockPreview::new(orgs, users, probe.actor_id, json!({"orders":ids}));
        let mut plan = OperationLockPlan::new(MarketOperation::Cancel);
        plan.parties(&parties)?;
        plan.slices([SliceKey {
            product_id: slice.product_id,
            delivery_point_id: Some(slice.delivery_point_id),
            availability_window: slice.availability_window.clone(),
        }])?;
        plan.orders(ids.clone())?;
        let locked = plan.acquire(&mut tx).await?;
        let policy = policy()?;
        let config = config()?;
        let at = Utc::now();
        let before = EffectPort::new(&mut tx, &ids, &locked)
            .capture_before(&policy, &config, &ids, at)
            .await?;
        let expiry_slice = super::super::ExpirySlice {
            product_id: slice.product_id,
            delivery_point_id: Some(slice.delivery_point_id),
            availability_window: &slice.availability_window,
            side: "ASK",
        };
        let facts = {
            let mut port = LockedMarketTransaction::from_effect_fixture(&mut tx, &plan, &locked);
            port.stock_ledger()?.expire_slice(&expiry_slice, at).await?
        };
        let mut descriptors = Vec::new();
        for fact in &facts {
            let mut next = probe.clone();
            next.order_id = fact.order_id;
            next.organization_id = fact.organization_id;
            let mut descriptor = effect(&mut tx, &next, &policy, at).await?;
            descriptor.changes = json!({"status":"EXPIRED","released_quantity_mt":fact.remaining_quantity_mt.value().to_string()});
            descriptors.push(descriptor);
        }
        let mut set = before.stage();
        set.expired(facts, descriptors)?;
        EffectPort::new(&mut tx, &ids, &locked)
            .finish(&policy, &config, set)
            .await?;
        let result = batch_result(pool, &mut tx, &locked).await?;
        if commit {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }
}

pub(super) async fn notify_trade_initiator(
    conn: &mut PgConnection,
    initiator: Uuid,
    trade_id: Uuid,
    trade: &Value,
    confirm: bool,
    at: DateTime<Utc>,
) -> Result<(), MarketWriteError> {
    let recipients: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
            .bind(initiator)
            .fetch_all(&mut *conn)
            .await?;
    for recipient in recipients {
        sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) VALUES($1,$2,'ORDER_UPDATE',$3,$4,$5,false,$6)").bind(Uuid::new_v4()).bind(recipient).bind(if confirm{"Trade Confirmed"}else{"Trade Declined"}).bind(format!("Your trade for {} MT has been {} by the counterparty.",trade["quantity_mt"].as_str().unwrap_or_default(),if confirm{"confirmed"}else{"declined"})).bind(json!({"trade_id":trade_id})).bind(at).execute(&mut *conn).await?;
    }
    Ok(())
}

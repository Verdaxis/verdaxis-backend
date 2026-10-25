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
    OrderCancelled,
    OrderExpired,
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
}

impl MutationKind {
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::OrderCancelled => "order.cancelled",
            Self::OrderExpired => "order.expired",
            Self::InventoryCreated => "inventory.created",
            Self::InventoryUpdated => "inventory.updated",
            Self::InventoryDeleted => "inventory.deleted",
            Self::InventoryPublished => "inventory.published",
            Self::SupportContextStarted => "market_support.context_started",
            Self::SupportContextExited => "market_support.context_exited",
            Self::SupportContextRevoked => "market_support.context_revoked",
            Self::SupportAuthorizationCreated => "market_support.authorization_created",
            Self::SupportAuthorizationRevoked => "market_support.authorization_revoked",
            Self::SupportCapabilityGranted => "market_support.capability_granted",
            Self::SupportCapabilityRevoked => "market_support.capability_revoked",
            Self::CommissionChanged => "commission.updated",
        }
    }
}

pub struct MutationEffect {
    pub kind: MutationKind,
    pub actor_id: Uuid,
    pub organization_id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub changes: Value,
    pub notification: OrganizationNotification,
    pub event: ParticipantEvent,
    pub at: DateTime<Utc>,
}

pub struct OrganizationNotification {
    pub kind: String,
    pub title: String,
    pub message: String,
    pub data: Value,
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
    slice: Slice,
    version: i32,
    status: String,
    side: String,
    price: Decimal,
    quantity: Decimal,
    remaining: Decimal,
    point_name: String,
    public: Option<Value>,
    qualified: bool,
    provenance: String,
}

impl OrderState {
    fn evidence(&self) -> Value {
        json!({"order_id":self.id,"version":self.version,"status":self.status,
            "side":self.side,"product_id":self.slice.product_id,"market_product":self.slice.product,
            "delivery_point_id":self.slice.point,"availability_window":self.slice.window,
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
    aggregates: Vec<Value>,
    targets: Vec<Value>,
    at: DateTime<Utc>,
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

    pub async fn capture_before(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        ids: &[Uuid],
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
        let slices = slices(&orders);
        let aggregates = aggregates(self.conn, policy, config, &slices, at).await?;
        // One statement snapshot defines event-time subscriptions for this mutation.
        let slice_json = slice_values(&slices);
        let rows = sqlx::query("SELECT t.id,t.watchlist_id,w.user_id,t.target_type,t.order_id,t.market_product_code,t.delivery_point_id,t.availability_window_code FROM watchlist_targets t JOIN watchlists w ON w.id=t.watchlist_id WHERE (t.target_type='PIN' AND t.order_id=ANY($1)) OR (t.target_type='SLICE' AND EXISTS (SELECT 1 FROM jsonb_to_recordset($2::jsonb) AS s(product text,point uuid,window_code text) WHERE s.product=t.market_product_code AND s.point=t.delivery_point_id AND s.window_code=t.availability_window_code)) ORDER BY t.id")
            .bind(&ids).bind(slice_json).fetch_all(&mut *self.conn).await?;
        let targets = rows.iter().map(|r| -> Result<Value,sqlx::Error> {
            Ok(json!({"target_id":r.try_get::<Uuid,_>("id")?,"watchlist_id":r.try_get::<Uuid,_>("watchlist_id")?,
                "observer_id":r.try_get::<Uuid,_>("user_id")?,"target_type":r.try_get::<String,_>("target_type")?,
                "order_id":r.try_get::<Option<Uuid>,_>("order_id")?,"market_product":r.try_get::<String,_>("market_product_code")?,
                "delivery_point_id":r.try_get::<Uuid,_>("delivery_point_id")?,"availability_window":r.try_get::<String,_>("availability_window_code")?}))
        }).collect::<Result<Vec<_>,_>>()?;
        Ok(BeforeEffects {
            orders,
            aggregates,
            targets,
            at,
        })
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
        if !matches!(effect.kind, MutationKind::OrderExpired)
            || facts.len() != before.orders.len()
            || facts.iter().any(|fact| {
                !before.orders.iter().any(|o| {
                    o.id == fact.order_id
                        && o.version == fact.previous_version
                        && o.status == fact.previous_status
                        && o.remaining == fact.remaining_quantity_mt.value()
                        && fact.previous_version.checked_add(1) == Some(fact.version)
                })
            })
        {
            return Err(inconsistent());
        }
        let ids: Vec<_> = before.orders.iter().map(|o| o.id).collect();
        let after = load_orders(self.conn, policy, config, &ids, effect.at).await?;
        if facts.iter().any(|fact| {
            !after.iter().any(|o| {
                o.id == fact.order_id && o.version == fact.version && o.status == "EXPIRED"
            })
        }) {
            return Err(inconsistent());
        }
        self.apply(policy, config, before, effect).await
    }

    async fn apply(
        &mut self,
        policy: &MarketPolicy,
        config: &Config,
        before: BeforeEffects,
        effect: MutationEffect,
    ) -> Result<Uuid, MarketWriteError> {
        self.locked.require_economic_phase()?;
        if before.at != effect.at {
            return Err(inconsistent());
        }
        let ids: Vec<_> = before.orders.iter().map(|o| o.id).collect();
        let after = load_orders(self.conn, policy, config, &ids, effect.at).await?;
        if before
            .orders
            .iter()
            .zip(&after)
            .any(|(a, b)| a.id != b.id || a.slice != b.slice)
        {
            return Err(inconsistent());
        }
        let slices = slices(&after);
        let aggregate_after = aggregates(self.conn, policy, config, &slices, effect.at).await?;
        rebuild_benchmarks(self.conn, &aggregate_after, effect.at).await?;
        let orders=before.orders.iter().zip(&after).enumerate().map(|(index,(a,b))|json!({"transition_index":index,"before":a.evidence(),"after":b.evidence()})).collect();
        append_audit(
            self.conn,
            &AuditFact {
                id: Uuid::new_v4(),
                actor_id: Some(effect.actor_id),
                action: effect.kind.audit_action().into(),
                resource_type: effect.resource_type,
                resource_id: Some(effect.resource_id.to_string()),
                changes: Some(effect.changes),
                ip_address: None,
                request_id: None,
                at: effect.at,
            },
        )
        .await?;
        sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) SELECT gen_random_uuid(),id,$2,$3,$4,$5::json,false,$6 FROM users WHERE organization_id=$1 ORDER BY id")
            .bind(effect.organization_id).bind(&effect.notification.kind).bind(&effect.notification.title).bind(&effect.notification.message).bind(&effect.notification.data).bind(effect.at).execute(&mut *self.conn).await?;
        // Terminal lock phase: never acquire an economic row after this call.
        self.locked.begin_target_phase()?;
        sync_pin_display(self.conn, &after).await?;
        let evidence = ProducerEvidence {
            mutation_id: Uuid::new_v4(),
            captured_at: effect.at,
            orders,
            aggregates_before: before.aggregates,
            aggregates_after: aggregate_after,
            targets: before.targets,
        };
        super::outbox::append(self.conn, &effect.event, &evidence).await
    }
}

fn inconsistent() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::InventoryInconsistent)
}
fn internal() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn slices(orders: &[OrderState]) -> BTreeSet<Slice> {
    orders.iter().map(|o| o.slice.clone()).collect()
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
    let rows=sqlx::query("SELECT o.id,o.product_id,o.delivery_point_id,o.availability_window,o.version,o.status::text AS status,o.side::text AS side,o.price_per_mt_usd,o.quantity_mt,o.remaining_quantity_mt,o.provenance::text AS provenance,dp.name AS point_name FROM orderbook_orders o JOIN delivery_points dp ON dp.id=o.delivery_point_id WHERE o.id=ANY($1) ORDER BY o.id")
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
                .ok_or_else(inconsistent)?;
            let window: String = r.try_get("availability_window")?;
            let admitted = public.remove(&id);
            Ok(OrderState {
                id,
                slice: Slice {
                    product_id,
                    product: product.code.clone(),
                    point: r.try_get("delivery_point_id")?,
                    window: normalize_window(&window, at.date_naive())
                        .map_err(|_| inconsistent())?,
                },
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
        sqlx::query("UPDATE watchlist_targets SET snapshot_price_per_mt_usd=$3::numeric::double precision,snapshot_quantity_mt=$4::numeric::double precision,snapshot_remaining_quantity_mt=$5::numeric::double precision,snapshot_status=$6,snapshot_side=$7,snapshot_market_product=$8,snapshot_delivery_point_name=$9,snapshot_availability_window=$10,snapshot_counterparty_label=NULL,market_product_code=$8,delivery_point_id=$11,availability_window_code=$10 WHERE id=$1 AND target_type='PIN' AND order_id=$2")
            .bind(id).bind(order.id).bind(order.price).bind(order.quantity).bind(order.remaining).bind(&order.status).bind(&order.side).bind(&order.slice.product).bind(&order.point_name).bind(&order.slice.window).bind(order.slice.point).execute(&mut *conn).await?;
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
    fn metadata(kind: MutationKind) -> (&'static str, &'static str, &'static str) {
        match kind {
            MutationKind::OrderCancelled => ("order_cancelled", "order", "CANCELLED"),
            MutationKind::OrderExpired => ("order_expired", "order", "EXPIRED"),
            MutationKind::InventoryCreated => ("inventory_created", "inventory", "created"),
            MutationKind::InventoryUpdated => ("inventory_updated", "inventory", "updated"),
            MutationKind::InventoryDeleted => ("inventory_deleted", "inventory", "deleted"),
            MutationKind::InventoryPublished => ("inventory_published", "inventory", "published"),
            MutationKind::SupportCapabilityGranted => {
                ("capability_granted", "staff_capability_assignment", "ACTIVE")
            }
            MutationKind::SupportCapabilityRevoked => {
                ("capability_revoked", "staff_capability_assignment", "REVOKED")
            }
            MutationKind::SupportContextStarted => ("context_started", "market_support_context", "ACTIVE"),
            MutationKind::SupportContextExited => ("context_exited", "market_support_context", "EXITED"),
            MutationKind::SupportContextRevoked => ("context_revoked", "market_support_context", "REVOKED"),
            MutationKind::SupportAuthorizationCreated => {
                ("authorization_created", "market_support_authorization", "ACTIVE")
            }
            MutationKind::SupportAuthorizationRevoked => {
                ("authorization_revoked", "market_support_authorization", "REVOKED")
            }
            MutationKind::CommissionChanged => ("commission_updated", "commission", "PAID"),
        }
    }
    fn effect(probe: &EffectProbe, at: DateTime<Utc>) -> MutationEffect {
        let (event_type, resource, status) = metadata(probe.kind);
        let fields = match resource {
            "order" => ParticipantFields::Order {
                order_id: probe.order_id,
                status: status.into(),
            },
            "inventory" => ParticipantFields::Inventory {
                inventory_id: probe.order_id,
                action: status.into(),
            },
            "commission" => ParticipantFields::Commission {
                commission_id: probe.order_id,
                status: status.into(),
            },
            _ => ParticipantFields::Support {
                context_id: probe.order_id,
                status: status.into(),
            },
        };
        MutationEffect {
            kind: probe.kind,
            actor_id: probe.actor_id,
            organization_id: probe.organization_id,
            resource_type: resource.into(),
            resource_id: probe.order_id,
            changes: json!({"status":{"before":"OPEN","after":status}}),
            notification: OrganizationNotification {
                kind: "ORDER_UPDATE".into(),
                title: "Market update".into(),
                message: "Market state changed".into(),
                data: json!({"action":probe.kind.audit_action(),"resource_id":probe.order_id}),
            },
            event: ParticipantEvent {
                event_type: event_type.into(),
                aggregate_type: resource.into(),
                aggregate_id: probe.order_id.to_string(),
                participant_org_ids: vec![probe.organization_id],
                fields,
                public_invalidated: resource == "order",
            },
            at,
        }
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
        let ids = if matches!(probe.kind, MutationKind::OrderCancelled) {
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
        apply_port(
            &mut tx.effects()?,
            &policy,
            &config,
            before,
            effect(probe, at),
        )
        .await?;
        Ok(())
    }
    pub async fn run_effect_probe(
        pool: &sqlx::PgPool,
        probe: EffectProbe,
    ) -> Result<ProbeResult, MarketWriteError> {
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
            MarketAttemptError::Database(e) => e.into(),
            MarketAttemptError::Refusal(e) => e,
            MarketAttemptError::PreviewChanged => {
                MarketWriteError::new(MarketErrorCode::MarketSliceChanged)
            }
        }
    }
    fn cancel_request_hash() -> String {
        super::super::command_request_hash(&super::super::Canonical::Object(BTreeMap::from([
            ("operation".into(), super::super::Canonical::Text("order.cancel".into())),
        ])))
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
                let response = CommittedCommand {
                    response_status: 204,
                    response_body: Value::Null,
                    replayed: false,
                };
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
        port.effects()?
            .orders_expired(&policy, &config, before, &facts, effect(&probe, at))
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
}

//! Caller-owned audit, notification, benchmark, PIN display and outbox effects.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::{Config, MarketPolicy};
use crate::business::audit::{AuditFact, append_audit};
use crate::market::live_benchmarks::{candidate_is_benchmark_qualified, qualified_vwap};
use crate::market::order_projection::PublicOrder;
use crate::market::visibility::{OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql};
use crate::market::windows::normalize_window;
use super::outbox::{ParticipantEvent, ProducerEvidence};
use super::{ExpiryEffect, MarketErrorCode, MarketWriteError};

#[derive(Clone, Copy, Debug)]
pub enum MutationKind {
    OrderCancelled,
    OrderExpired,
    InventoryPublished,
    SupportContextStarted,
    SupportContextExited,
    SupportCapabilityGranted,
    SupportCapabilityRevoked,
    CommissionChanged,
}

impl MutationKind {
    pub fn audit_action(self) -> &'static str {
        match self {
            Self::OrderCancelled => "order.cancelled",
            Self::OrderExpired => "order.expired",
            Self::InventoryPublished => "inventory.published",
            Self::SupportContextStarted => "market_support.context_started",
            Self::SupportContextExited => "market_support.context_exited",
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
}

impl<'a> EffectPort<'a> {
    pub(super) fn new(conn: &'a mut PgConnection, order_ids: &'a [Uuid]) -> Self {
        Self { conn, order_ids }
    }

    pub async fn capture_before(&mut self, policy: &MarketPolicy, config: &Config, ids: &[Uuid], at: DateTime<Utc>) -> Result<BeforeEffects, MarketWriteError> {
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
        let rows = sqlx::query("SELECT t.id,t.watchlist_id,w.user_id,t.target_type,t.order_id,t.market_product_code,t.delivery_point_id,t.availability_window_code FROM watchlist_targets t JOIN watchlists w ON w.id=t.watchlist_id WHERE (t.target_type='PIN' AND t.order_id=ANY($1)) OR (t.target_type='SLICE' AND EXISTS (SELECT 1 FROM jsonb_to_recordset($2::jsonb) AS s(product text,point uuid,window text) WHERE s.product=t.market_product_code AND s.point=t.delivery_point_id AND s.window=t.availability_window_code)) ORDER BY t.id")
            .bind(&ids).bind(slice_json).fetch_all(&mut *self.conn).await?;
        let targets = rows.iter().map(|r| -> Result<Value,sqlx::Error> {
            Ok(json!({"target_id":r.try_get::<Uuid,_>("id")?,"watchlist_id":r.try_get::<Uuid,_>("watchlist_id")?,
                "observer_id":r.try_get::<Uuid,_>("user_id")?,"target_type":r.try_get::<String,_>("target_type")?,
                "order_id":r.try_get::<Option<Uuid>,_>("order_id")?,"market_product":r.try_get::<String,_>("market_product_code")?,
                "delivery_point_id":r.try_get::<Uuid,_>("delivery_point_id")?,"availability_window":r.try_get::<String,_>("availability_window_code")?}))
        }).collect::<Result<Vec<_>,_>>()?;
        Ok(BeforeEffects { orders, aggregates, targets, at })
    }

    pub async fn order_cancelled(&mut self, policy: &MarketPolicy, config: &Config, before: BeforeEffects, effect: MutationEffect) -> Result<Uuid,MarketWriteError> {
        if !matches!(effect.kind, MutationKind::OrderCancelled) { return Err(inconsistent()); }
        self.apply(policy,config,before,effect).await
    }

    pub async fn inventory_changed(&mut self, policy: &MarketPolicy, config: &Config, before: BeforeEffects, effect: MutationEffect) -> Result<Uuid,MarketWriteError> {
        if !matches!(effect.kind, MutationKind::InventoryPublished) { return Err(inconsistent()); }
        self.apply(policy,config,before,effect).await
    }

    pub async fn support_changed(&mut self, policy: &MarketPolicy, config: &Config, before: BeforeEffects, effect: MutationEffect) -> Result<Uuid,MarketWriteError> {
        if !matches!(effect.kind, MutationKind::SupportContextStarted|MutationKind::SupportContextExited|MutationKind::SupportCapabilityGranted|MutationKind::SupportCapabilityRevoked) { return Err(inconsistent()); }
        self.apply(policy,config,before,effect).await
    }

    pub async fn commission_changed(&mut self, policy: &MarketPolicy, config: &Config, before: BeforeEffects, effect: MutationEffect) -> Result<Uuid,MarketWriteError> {
        if !matches!(effect.kind, MutationKind::CommissionChanged) { return Err(inconsistent()); }
        self.apply(policy,config,before,effect).await
    }

    /// The expiry facts bind the version transition; visibility was captured
    /// before the stock primitive changed these already locked orders.
    pub async fn orders_expired(&mut self, policy: &MarketPolicy, config: &Config, before: BeforeEffects, facts: &[ExpiryEffect], effect: MutationEffect) -> Result<Uuid,MarketWriteError> {
        if !matches!(effect.kind,MutationKind::OrderExpired) || facts.len()!=before.orders.len()
            || facts.iter().any(|fact| !before.orders.iter().any(|o| o.id==fact.order_id && o.version==fact.previous_version && o.status==fact.previous_status && o.remaining==fact.remaining_quantity_mt.value() && fact.previous_version.checked_add(1)==Some(fact.version))) {
            return Err(inconsistent());
        }
        let ids: Vec<_> = before.orders.iter().map(|o|o.id).collect();
        let after=load_orders(self.conn,policy,config,&ids,effect.at).await?;
        if facts.iter().any(|fact| !after.iter().any(|o|o.id==fact.order_id && o.version==fact.version && o.status=="EXPIRED")) { return Err(inconsistent()); }
        self.apply(policy,config,before,effect).await
    }

    async fn apply(&mut self, policy: &MarketPolicy, config: &Config, before: BeforeEffects, effect: MutationEffect) -> Result<Uuid,MarketWriteError> {
        if before.at!=effect.at { return Err(inconsistent()); }
        let ids: Vec<_> = before.orders.iter().map(|o|o.id).collect();
        let after=load_orders(self.conn,policy,config,&ids,effect.at).await?;
        if before.orders.iter().zip(&after).any(|(a,b)|a.id!=b.id || a.slice!=b.slice) { return Err(inconsistent()); }
        let slices=slices(&after);
        let aggregate_after=aggregates(self.conn,policy,config,&slices,effect.at).await?;
        rebuild_benchmarks(self.conn,&aggregate_after,effect.at).await?;
        let orders=before.orders.iter().zip(&after).enumerate().map(|(index,(a,b))|json!({"transition_index":index,"before":a.evidence(),"after":b.evidence()})).collect();
        append_audit(self.conn,&AuditFact {id:Uuid::new_v4(),actor_id:Some(effect.actor_id),action:effect.kind.audit_action().into(),resource_type:effect.resource_type,resource_id:Some(effect.resource_id.to_string()),changes:Some(effect.changes),ip_address:None,request_id:None,at:effect.at}).await?;
        sqlx::query("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) SELECT gen_random_uuid(),id,$2,$3,$4,$5::json,false,$6 FROM users WHERE organization_id=$1 ORDER BY id")
            .bind(effect.organization_id).bind(&effect.notification.kind).bind(&effect.notification.title).bind(&effect.notification.message).bind(&effect.notification.data).bind(effect.at).execute(&mut *self.conn).await?;
        // Terminal lock phase: never acquire an economic row after this call.
        sync_pin_display(self.conn,&after).await?;
        let evidence=ProducerEvidence {mutation_id:Uuid::new_v4(),captured_at:effect.at,orders,aggregates_before:before.aggregates,aggregates_after:aggregate_after,targets:before.targets};
        super::outbox::append(self.conn,&effect.event,&evidence).await
    }
}

fn inconsistent() -> MarketWriteError { MarketWriteError::new(MarketErrorCode::InventoryInconsistent) }
fn internal() -> MarketWriteError { MarketWriteError::new(MarketErrorCode::DatabaseError) }
fn slices(orders: &[OrderState]) -> BTreeSet<Slice> { orders.iter().map(|o|o.slice.clone()).collect() }
fn slice_values(slices: &BTreeSet<Slice>) -> Value { json!(slices.iter().map(|s|json!({"product":s.product,"point":s.point,"window":s.window})).collect::<Vec<_>>()) }

async fn load_orders(conn: &mut PgConnection,policy: &MarketPolicy,config: &Config,ids: &[Uuid],at: DateTime<Utc>) -> Result<Vec<OrderState>,MarketWriteError> {
    let clock=VisibilityClock::at(at);
    let sql=order_query_sql("SELECT e.*, /* private_digest_input(e) */ AS private_digest_input FROM eligible_orders e WHERE e.id=ANY($14)");
    let rows=bind_order_query(&sql,policy,&clock,config,&PublicOrderFilters::default())?.bind(ids).fetch_all(&mut *conn).await?;
    let mut public=BTreeMap::new();
    for row in rows {
        let projection=PublicOrder::from_row(&row).map_err(|_|internal())?;
        let qualified=candidate_is_benchmark_qualified(policy,&projection.private,&clock);
        public.insert(row.try_get::<Uuid,_>("id")?,(projection.public(false,None).map_err(|_|internal())?,qualified));
    }
    let rows=sqlx::query("SELECT o.id,o.product_id,o.delivery_point_id,o.availability_window,o.version,o.status::text AS status,o.side::text AS side,o.price_per_mt_usd,o.quantity_mt,o.remaining_quantity_mt,o.provenance::text AS provenance,dp.name AS point_name FROM orderbook_orders o JOIN delivery_points dp ON dp.id=o.delivery_point_id WHERE o.id=ANY($1) ORDER BY o.id")
        .bind(ids).fetch_all(conn).await?;
    if rows.len()!=ids.len() { return Err(inconsistent()); }
    rows.iter().map(|r| {
        let id:Uuid=r.try_get("id")?;
        let product_id:Uuid=r.try_get("product_id")?;
        let product=policy.products.iter().find(|p|p.id==product_id).ok_or_else(inconsistent)?;
        let window:String=r.try_get("availability_window")?;
        let admitted=public.remove(&id);
        Ok(OrderState {id,slice:Slice {product_id,product:product.code.clone(),point:r.try_get("delivery_point_id")?,window:normalize_window(&window,at.date_naive()).map_err(|_|inconsistent())?},version:r.try_get("version")?,status:r.try_get("status")?,side:r.try_get("side")?,price:r.try_get("price_per_mt_usd")?,quantity:r.try_get("quantity_mt")?,remaining:r.try_get("remaining_quantity_mt")?,point_name:r.try_get("point_name")?,qualified:admitted.as_ref().is_some_and(|(_,q)|*q),public:admitted.map(|(p,_)|p),provenance:r.try_get("provenance")?})
    }).collect()
}

async fn aggregates(conn: &mut PgConnection,policy: &MarketPolicy,config: &Config,slices: &BTreeSet<Slice>,at:DateTime<Utc>) -> Result<Vec<Value>,MarketWriteError> {
    let mut values=Vec::new();
    let clock=VisibilityClock::at(at);
    for slice in slices {
        for side in ["BID","ASK"] {
            let sql=order_query_sql("SELECT e.remaining_quantity_mt,e.price_per_mt_usd,e.provenance::text AS provenance, /* private_digest_input(e) */ AS private_digest_input FROM eligible_orders e");
            let filters=PublicOrderFilters {surface:OrderSurface::Candidates,product_id:Some(slice.product_id),delivery_point_id:Some(slice.point),availability_window:Some(&slice.window),side:Some(side),..Default::default()};
            let rows=bind_order_query(&sql,policy,&clock,config,&filters)?.fetch_all(&mut *conn).await?;
            let mut economic=Vec::new();
            let mut real=0usize;
            for row in rows {
                let private:Value=row.try_get("private_digest_input")?;
                if candidate_is_benchmark_qualified(policy,&private,&clock) {
                    if row.try_get::<String,_>("provenance")?=="REAL" { real+=1; }
                    economic.push((row.try_get::<Decimal,_>("remaining_quantity_mt")?,row.try_get::<Decimal,_>("price_per_mt_usd")?));
                }
            }
            let quantity=economic.iter().try_fold(Decimal::ZERO,|sum,(q,_)|sum.checked_add(*q).ok_or_else(inconsistent))?;
            let best=economic.iter().map(|(_,p)|*p).reduce(|a,b|if side=="BID" {a.max(b)}else {a.min(b)});
            let vwap=qualified_vwap(&economic);
            values.push(json!({"market_product":slice.product,"delivery_point_id":slice.point,"availability_window":slice.window,"side":side,"quantity_mt":quantity.to_string(),"order_count":economic.len(),"real_count":real,"demo_count":economic.len()-real,"best_price_per_mt_usd":best.map(|p|p.to_string()),"benchmark_price_per_mt_usd":vwap.map(|p|p.to_string())}));
        }
    }
    Ok(values)
}

async fn rebuild_benchmarks(conn:&mut PgConnection,values:&[Value],at:DateTime<Utc>) -> Result<(),MarketWriteError> {
    for value in values {
        let text=|name|value[name].as_str().ok_or_else(inconsistent);
        let point:Uuid=text("delivery_point_id")?.parse().map_err(|_|inconsistent())?;
        sqlx::query("DELETE FROM live_slice_benchmarks WHERE side=$1 AND market_product=$2 AND delivery_point_id=$3 AND availability_window=$4")
            .bind(text("side")?).bind(text("market_product")?).bind(point).bind(text("availability_window")?).execute(&mut *conn).await?;
        if let Some(price)=value["benchmark_price_per_mt_usd"].as_str() {
            sqlx::query("INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'live_slice_vwap',$9,$9)")
                .bind(Uuid::new_v4()).bind(text("side")?).bind(text("market_product")?).bind(point).bind(text("availability_window")?).bind(price.parse::<Decimal>().map_err(|_|inconsistent())?).bind(text("quantity_mt")?.parse::<Decimal>().map_err(|_|inconsistent())?).bind(i32::try_from(value["order_count"].as_u64().ok_or_else(inconsistent)?).map_err(|_|inconsistent())?).bind(at).execute(&mut *conn).await?;
        }
    }
    Ok(())
}

async fn sync_pin_display(conn:&mut PgConnection,orders:&[OrderState]) -> Result<(),MarketWriteError> {
    let ids:Vec<_>=orders.iter().map(|o|o.id).collect();
    let targets=sqlx::query("SELECT id,order_id FROM watchlist_targets WHERE target_type='PIN' AND order_id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(&ids).fetch_all(&mut *conn).await?;
    for target in targets {
        let id:Uuid=target.try_get("id")?;
        let order_id:Uuid=target.try_get("order_id")?;
        let order=orders.iter().find(|o|o.id==order_id).ok_or_else(inconsistent)?;
        sqlx::query("UPDATE watchlist_targets SET snapshot_price_per_mt_usd=$3::numeric::double precision,snapshot_quantity_mt=$4::numeric::double precision,snapshot_remaining_quantity_mt=$5::numeric::double precision,snapshot_status=$6,snapshot_side=$7,snapshot_market_product=$8,snapshot_delivery_point_name=$9,snapshot_availability_window=$10,snapshot_counterparty_label=NULL,market_product_code=$8,delivery_point_id=$11,availability_window_code=$10 WHERE id=$1 AND target_type='PIN' AND order_id=$2")
            .bind(id).bind(order.id).bind(order.price).bind(order.quantity).bind(order.remaining).bind(&order.status).bind(&order.side).bind(&order.slice.product).bind(&order.point_name).bind(&order.slice.window).bind(order.slice.point).execute(&mut *conn).await?;
    }
    Ok(())
}

#[cfg(feature="contract-tests")]
pub mod contract {
    use super::*;
    use super::super::outbox::ParticipantFields;
    pub struct EffectProbe { pub order_id:Uuid,pub actor_id:Uuid,pub organization_id:Uuid,pub kind:MutationKind,pub replay:bool,pub commit:bool }
    pub struct ProbeResult { pub visible_from_writer:i64,pub visible_from_observer:i64,pub writer_pin_status:String,pub observer_pin_status:String }
    pub async fn run_effect_probe(pool:&sqlx::PgPool,probe:EffectProbe) -> Result<ProbeResult,MarketWriteError> {
        let policy:MarketPolicy=serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json")).map_err(|_|internal())?;
        let config:Config=serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).map_err(|_|internal())?;
        let at=Utc::now();
        let mut tx=pool.begin().await?;
        sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE").bind(probe.organization_id).fetch_one(&mut *tx).await?;
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(probe.actor_id).fetch_one(&mut *tx).await?;
        sqlx::query("SELECT id FROM orderbook_orders WHERE id=$1 FOR UPDATE").bind(probe.order_id).fetch_one(&mut *tx).await?;
        let ids=[probe.order_id];
        if !probe.replay {
            let before=EffectPort::new(&mut tx,&ids).capture_before(&policy,&config,&ids,at).await?;
            if matches!(probe.kind,MutationKind::OrderCancelled) { sqlx::query("UPDATE orderbook_orders SET status='CANCELLED',version=version+1 WHERE id=$1").bind(probe.order_id).execute(&mut *tx).await?; }
            let (event_type,fields)=match probe.kind {
                MutationKind::OrderCancelled => ("order_cancelled",ParticipantFields::Order {order_id:probe.order_id,status:"CANCELLED".into()}),
                MutationKind::InventoryPublished => ("inventory_published",ParticipantFields::Inventory {inventory_id:probe.order_id,action:"published".into()}),
                MutationKind::SupportContextStarted => ("context_started",ParticipantFields::Support {context_id:probe.order_id,status:"ACTIVE".into()}),
                _ => ("commission_updated",ParticipantFields::Commission {commission_id:probe.order_id,status:"PAID".into()}),
            };
            let effect=MutationEffect {kind:probe.kind,actor_id:probe.actor_id,organization_id:probe.organization_id,resource_type:"order".into(),resource_id:probe.order_id,changes:json!({"status":{"before":"OPEN","after":"CANCELLED"}}),notification:OrganizationNotification {kind:"ORDER_UPDATE".into(),title:"Order update".into(),message:"Order state changed".into(),data:json!({"order_id":probe.order_id})},event:ParticipantEvent {event_type:event_type.into(),aggregate_type:"order".into(),aggregate_id:probe.order_id.to_string(),participant_org_ids:vec![probe.organization_id],fields,public_invalidated:matches!(probe.kind,MutationKind::OrderCancelled)},at};
            let mut port=EffectPort::new(&mut tx,&ids);
            match probe.kind {
                MutationKind::OrderCancelled => port.order_cancelled(&policy,&config,before,effect).await?,
                MutationKind::InventoryPublished => port.inventory_changed(&policy,&config,before,effect).await?,
                MutationKind::SupportContextStarted => port.support_changed(&policy,&config,before,effect).await?,
                _ => port.commission_changed(&policy,&config,before,effect).await?,
            };
        }
        let visible_from_writer=sqlx::query_scalar("SELECT count(*) FROM market_event_outbox").fetch_one(&mut *tx).await?;
        let writer_pin_status=sqlx::query_scalar("SELECT snapshot_status FROM watchlist_targets WHERE order_id=$1").bind(probe.order_id).fetch_one(&mut *tx).await?;
        let mut observer=pool.acquire().await?;
        let visible_from_observer=sqlx::query_scalar("SELECT count(*) FROM market_event_outbox").fetch_one(&mut *observer).await?;
        let observer_pin_status=sqlx::query_scalar("SELECT snapshot_status FROM watchlist_targets WHERE order_id=$1").bind(probe.order_id).fetch_one(&mut *observer).await?;
        if probe.commit { tx.commit().await?; } else { tx.rollback().await?; }
        Ok(ProbeResult {visible_from_writer,visible_from_observer,writer_pin_status,observer_pin_status})
    }
}

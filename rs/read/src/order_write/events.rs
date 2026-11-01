use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::invalidation::{InvalidateOrganization, InvalidationError, OrderRestoration};
use crate::{
    business::audit::{AuditFact, append_audit},
    market::visibility::{VisibilityClock, public_order_sql},
};

async fn audit(
    conn: &mut PgConnection,
    command: &InvalidateOrganization,
    action: &str,
    resource: &str,
    id: Uuid,
    changes: Value,
) -> Result<(), InvalidationError> {
    append_audit(
        conn,
        &AuditFact {
            id: Uuid::new_v4(),
            actor_id: Some(command.actor_id),
            action: action.into(),
            resource_type: resource.into(),
            resource_id: Some(id.to_string()),
            changes: Some(changes),
            ip_address: None,
            request_id: None,
            at: command.now,
        },
    )
    .await?;
    Ok(())
}

/// An exact JSON number for a `numeric` value. `Decimal` serializes as a string
/// and `f64` would round, so the decimal text becomes the number itself
/// (`serde_json` keeps number text as written through `arbitrary_precision`).
fn json_number(value: Decimal) -> Result<Value, sqlx::Error> {
    value
        .to_string()
        .parse::<serde_json::Number>()
        .map(Value::Number)
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))
}

pub(super) async fn restoration_effects(
    conn: &mut PgConnection,
    command: &InvalidateOrganization,
    changes: &[OrderRestoration],
    trades: &[Uuid],
) -> Result<(), InvalidationError> {
    let reference = format!("organization:{}", command.organization_id);
    for trade in trades {
        audit(
            conn,
            command,
            "trade.cancelled",
            "trade",
            *trade,
            json!({"status":"CANCELLED","reason":"organization_rejected","reference":reference}),
        )
        .await?;
    }
    for change in changes {
        let order = &change.after;
        let product = command
            .policy
            .products
            .iter()
            .find(|p| p.id == order.product_id)
            .map(|p| p.code.as_str());
        let point_name = command
            .policy
            .points
            .iter()
            .find(|p| Some(p.id) == order.delivery_point_id)
            .map(|p| p.name.as_str());
        let (source, demo) = match order.provenance.as_str() {
            "REAL" => ("LIVE_ORDER", "REAL_ONLY"),
            "DEMO" => ("DEMO_SEED", "DEMO_ONLY"),
            _ => ("UNKNOWN", "UNKNOWN"),
        };
        let scope = if order.delivery_point_id.is_some() {
            "DELIVERY_POINT"
        } else {
            "UNKNOWN"
        };
        let mut payload = json!({"source_kind":source,"demo_status":demo,"scope":scope,"observed_at":command.now,"market_product_code":product,"delivery_point_id":order.delivery_point_id,"delivery_point_name":point_name,"availability_window_code":order.availability_window,"order_id":order.id,"status":order.status});
        // The quantities describe a remainder change. A status-only event leaves them out.
        let remainder_changed = change.restored != Decimal::ZERO;
        if remainder_changed {
            payload["old_remaining_quantity_mt"] =
                json_number(change.before.remaining_quantity_mt)?;
            payload["new_remaining_quantity_mt"] = json_number(order.remaining_quantity_mt)?;
        }
        let event = match order.status.as_str() {
            "CANCELLED" => "PIN_WITHDRAWN",
            "EXPIRED" => "PIN_EXPIRED",
            "PARTIALLY_FILLED" => "PIN_PARTIALLY_FILLED",
            "FILLED" => "PIN_FILLED",
            _ => "PIN_QUANTITY_CHANGED",
        };
        if remainder_changed || change.before.status != order.status {
            sqlx::query("INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at) SELECT gen_random_uuid(),watchlist_id,id,$4,$2::jsonb,false,$3 FROM watchlist_targets WHERE target_type='PIN' AND order_id=$1")
                .bind(order.id).bind(&payload).bind(command.now).bind(event).execute(&mut *conn).await?;
        }
        sqlx::query("UPDATE watchlist_targets SET market_product_code=$2,delivery_point_id=$3,availability_window_code=$4,snapshot_price_per_mt_usd=$5::numeric::double precision,snapshot_quantity_mt=$6::numeric::double precision,snapshot_remaining_quantity_mt=$7::numeric::double precision,snapshot_status=$10,snapshot_side=$8,snapshot_market_product=$2,snapshot_delivery_point_name=$9,snapshot_availability_window=$4,snapshot_counterparty_label=NULL WHERE target_type='PIN' AND order_id=$1")
            .bind(order.id).bind(product).bind(order.delivery_point_id).bind(&order.availability_window).bind(order.price_per_mt_usd).bind(order.quantity_mt).bind(order.remaining_quantity_mt).bind(&order.side).bind(point_name).bind(&order.status).execute(&mut *conn).await?;
        if let (Some(product), Some(point)) = (product, order.delivery_point_id) {
            // Watchlist evidence includes UNKNOWN counts, but never selects their prices.
            // Keep the canonical catalog/current/admission predicates unchanged.
            let relation = public_order_sql().replace(
                "AND o.provenance IN ('REAL', 'DEMO')",
                "AND o.provenance IN ('REAL', 'DEMO', 'UNKNOWN')",
            );
            let sql = format!(
                "WITH eligible_orders AS NOT MATERIALIZED ({relation}) SELECT provenance,CASE WHEN $7::text='ASK' THEN min(price_per_mt_usd) ELSE max(price_per_mt_usd) END AS price,count(*) AS count,max(coalesce(updated_at,created_at)) AS observed FROM eligible_orders WHERE NOT off_spec AND (side<>'ASK' OR (certification_declared AND length(btrim(certification_scheme))>0)) GROUP BY provenance"
            );
            let clock = VisibilityClock::at(command.now);
            let policy = serde_json::to_string(command.policy.as_ref())
                .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(policy)
                .bind(clock.now)
                .bind(clock.singapore_date.format("%Y-%m-%d").to_string())
                .bind(false)
                .bind(order.product_id)
                .bind(point)
                .bind(&order.side)
                .bind(Option::<String>::None)
                .bind(Option::<String>::None)
                .bind(Option::<String>::None)
                .bind(&order.availability_window)
                .bind("candidates")
                .bind(command.config.kyc_execution_gate_enabled)
                .fetch_all(&mut *conn)
                .await?;
            let selected = rows
                .iter()
                .find(|r| r.get::<String, _>("provenance") == "REAL")
                .or_else(|| {
                    rows.iter()
                        .find(|r| r.get::<String, _>("provenance") == "DEMO")
                });
            if let Some(row) = selected {
                let provenance: String = row.try_get("provenance")?;
                let price: Option<Decimal> = row.try_get("price")?;
                let observed: Option<DateTime<Utc>> = row.try_get("observed")?;
                let counts = |kind: &str| -> Result<i64, sqlx::Error> {
                    rows.iter()
                        .find(|r| r.get::<String, _>("provenance") == kind)
                        .map(|r| r.try_get("count"))
                        .transpose()
                        .map(|v| v.unwrap_or(0))
                };
                let real_count = counts("REAL")?;
                let demo_count = counts("DEMO")?;
                let unknown_count = counts("UNKNOWN")?;
                let (source, demo) = if provenance == "REAL" {
                    ("LIVE_ORDER", "REAL_ONLY")
                } else {
                    ("DEMO_SEED", "DEMO_ONLY")
                };
                let slice_payload = json!({"source_kind":source,"demo_status":demo,"scope":"DELIVERY_POINT","observed_at":observed,"market_product_code":product,"delivery_point_id":point,"availability_window_code":order.availability_window,"side":order.side,"old_price_per_mt_usd":null,"new_price_per_mt_usd":price,"real_count":real_count,"demo_count":demo_count,"unknown_count":unknown_count});
                sqlx::query("INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at) SELECT gen_random_uuid(),watchlist_id,id,'SLICE_BEST_PRICE_MOVED',$4::jsonb,false,$5 FROM watchlist_targets WHERE target_type='SLICE' AND market_product_code=$1 AND delivery_point_id=$2 AND availability_window_code=$3")
                    .bind(product).bind(point).bind(&order.availability_window).bind(slice_payload).bind(command.now).execute(&mut *conn).await?;
            }
        }
        let terminal_before = matches!(change.before.status.as_str(), "CANCELLED" | "EXPIRED");
        let action = if order.organization_id == command.organization_id && !terminal_before {
            "order.cancelled"
        } else if order.status == "EXPIRED" && !terminal_before {
            "order.expired"
        } else {
            "order.updated"
        };
        audit(conn,command,action,"order",order.id,json!({"status":order.status,"restored_quantity_mt":change.restored.to_string(),"released_quantity_mt":change.released.to_string(),"reason":"organization_rejected","reference":reference})).await?;
    }
    Ok(())
}

pub(super) async fn organization_effects(
    conn: &mut PgConnection,
    command: &InvalidateOrganization,
    orders: &[Uuid],
    trades: &[Uuid],
    participants: Vec<Uuid>,
) -> Result<Uuid, InvalidationError> {
    audit(
        conn,
        command,
        "market.access_invalidated",
        "organization",
        command.organization_id,
        json!({"cancelled_trade_ids":trades,"affected_order_ids":orders,"reason":"organization_rejected","reference":format!("organization:{}",command.organization_id)}),
    )
    .await?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at,delivery_attempts) VALUES($1,'market_access_invalidated','organization',$2,$3::jsonb,$4::jsonb,$5,0)")
        .bind(id).bind(command.organization_id.to_string()).bind(json!(participants)).bind(json!({"schema_version":1,"organization_id":command.organization_id,"cancelled_trade_count":trades.len(),"affected_order_count":orders.len(),"reference":format!("organization:{}",command.organization_id),"_public_market_invalidation":true})).bind(command.now).execute(&mut *conn).await?;
    super::transaction::wake_market_event_dispatchers(conn).await?;
    Ok(id)
}

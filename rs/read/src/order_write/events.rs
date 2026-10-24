use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::invalidation::{InvalidateOrganization, InvalidationError, Order};
use crate::{
    business::audit::{AuditFact, append_audit},
    market::visibility::{
        OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
    },
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

pub(super) async fn withdrawal_effects(
    conn: &mut PgConnection,
    command: &InvalidateOrganization,
    orders: &[Order],
) -> Result<(), InvalidationError> {
    for order in orders {
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
        let payload = json!({"source_kind":source,"demo_status":demo,"scope":"DELIVERY_POINT","observed_at":command.now,"market_product_code":product,"delivery_point_id":order.delivery_point_id,"delivery_point_name":point_name,"availability_window_code":order.availability_window,"order_id":order.id,"status":"CANCELLED"});
        sqlx::query("INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at) SELECT gen_random_uuid(),watchlist_id,id,'PIN_WITHDRAWN',$2::jsonb,false,$3 FROM watchlist_targets WHERE target_type='PIN' AND order_id=$1")
            .bind(order.id).bind(&payload).bind(command.now).execute(&mut *conn).await?;
        sqlx::query("UPDATE watchlist_targets SET market_product_code=$2,delivery_point_id=$3,availability_window_code=$4,snapshot_price_per_mt_usd=$5::numeric::double precision,snapshot_quantity_mt=$6::numeric::double precision,snapshot_remaining_quantity_mt=$7::numeric::double precision,snapshot_status='CANCELLED',snapshot_side=$8,snapshot_market_product=$2,snapshot_delivery_point_name=$9,snapshot_availability_window=$4,snapshot_counterparty_label=NULL WHERE target_type='PIN' AND order_id=$1")
            .bind(order.id).bind(product).bind(order.delivery_point_id).bind(&order.availability_window).bind(order.price_per_mt_usd).bind(order.quantity_mt).bind(order.remaining_quantity_mt).bind(&order.side).bind(point_name).execute(&mut *conn).await?;
        if let (Some(product), Some(point)) = (product, order.delivery_point_id) {
            // Reuse the canonical visibility relation. REAL evidence takes precedence over DEMO.
            let sql = order_query_sql(
                "SELECT provenance,CASE WHEN $7::text='ASK' THEN min(price_per_mt_usd) ELSE max(price_per_mt_usd) END AS price,count(*) AS count,max(coalesce(updated_at,created_at)) AS observed FROM eligible_orders WHERE NOT off_spec AND (side<>'ASK' OR (certification_declared AND length(btrim(certification_scheme))>0)) GROUP BY provenance",
            );
            let clock = VisibilityClock::at(command.now);
            let filters = PublicOrderFilters {
                surface: OrderSurface::Candidates,
                product_id: Some(order.product_id),
                delivery_point_id: Some(point),
                side: Some(&order.side),
                availability_window: Some(&order.availability_window),
                ..Default::default()
            };
            let rows = bind_order_query(&sql, &command.policy, &clock, &command.config, &filters)?
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
                let (source, demo) = if provenance == "REAL" {
                    ("LIVE_ORDER", "REAL_ONLY")
                } else {
                    ("DEMO_SEED", "DEMO_ONLY")
                };
                let slice_payload = json!({"source_kind":source,"demo_status":demo,"scope":"DELIVERY_POINT","observed_at":observed,"market_product_code":product,"delivery_point_id":point,"availability_window_code":order.availability_window,"side":order.side,"old_price_per_mt_usd":null,"new_price_per_mt_usd":price,"real_count":real_count,"demo_count":demo_count,"unknown_count":0});
                sqlx::query("INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at) SELECT gen_random_uuid(),watchlist_id,id,'SLICE_BEST_PRICE_MOVED',$4::jsonb,false,$5 FROM watchlist_targets WHERE target_type='SLICE' AND market_product_code=$1 AND delivery_point_id=$2 AND availability_window_code=$3")
                    .bind(product).bind(point).bind(&order.availability_window).bind(slice_payload).bind(command.now).execute(&mut *conn).await?;
            }
        }
        audit(conn,command,"order.cancelled","order",order.id,json!({"status":"CANCELLED","restored_quantity_mt":"0","released_quantity_mt":if order.inventory_item_id.is_some() {order.remaining_quantity_mt} else {Decimal::ZERO}})).await?;
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
        json!({"preserved_trade_ids":trades,"affected_order_ids":orders}),
    )
    .await?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at,delivery_attempts) VALUES($1,'market_access_invalidated','organization',$2,$3::jsonb,$4::jsonb,$5,0)")
        .bind(id).bind(command.organization_id.to_string()).bind(json!(participants)).bind(json!({"schema_version":1,"organization_id":command.organization_id,"cancelled_trade_count":0,"preserved_trade_count":trades.len(),"affected_order_count":orders.len(),"_public_market_invalidation":true})).bind(command.now).execute(conn).await?;
    Ok(id)
}

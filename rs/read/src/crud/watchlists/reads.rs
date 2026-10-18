//! Actor-scoped batched loading. The caller supplies the shared public-order
//! result captured on this transaction; no private visibility policy is copied.
use super::{Event, Metrics, Summary, Target, project_summary};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct VisibleOrder {
    pub market_product: String,
    pub delivery_point_id: Uuid,
    pub availability_window: String,
}

/// Four bounded statements independent of target count: owner, targets, events
/// and (by the caller) the shared public-order relation. No state is updated.
pub async fn load_detail(
    connection: &mut PgConnection,
    actor_id: Uuid,
    watchlist_id: Uuid,
    visible_order_ids: &HashSet<Uuid>,
    visible_orders: &[VisibleOrder],
    best_price_cutoff: DateTime<Utc>,
) -> Result<Option<Summary>, sqlx::Error> {
    let Some(container) = sqlx::query(
        "SELECT id,name,kind::text AS kind,created_at FROM watchlists WHERE id=$1 AND user_id=$2",
    )
    .bind(watchlist_id)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(None);
    };
    let rows=sqlx::query("SELECT (to_jsonb(t)||jsonb_build_object('delivery_point_name',COALESCE(p.name,t.snapshot_delivery_point_name)))::text AS value FROM watchlist_targets t LEFT JOIN delivery_points p ON p.id=t.delivery_point_id WHERE t.watchlist_id=$1 ORDER BY t.created_at DESC")
        .bind(watchlist_id).fetch_all(&mut *connection).await?;
    let targets = rows
        .iter()
        .map(|row| {
            let raw: String = row.try_get("value")?;
            serde_json::from_str::<Target>(&raw)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|target| {
            target.target_type == "SLICE"
                || target
                    .order_id
                    .is_some_and(|id| visible_order_ids.contains(&id))
        })
        .collect::<Vec<_>>();
    let rows=sqlx::query("SELECT watchlist_target_id,event_type::text AS event_type,event_payload::text AS payload,is_read,created_at FROM watchlist_events WHERE watchlist_id=$1")
        .bind(watchlist_id).fetch_all(&mut *connection).await?;
    let events = rows
        .iter()
        .map(|row| {
            let payload: String = row.try_get("payload")?;
            let payload: serde_json::Value = serde_json::from_str(&payload)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            let order_id = payload
                .get("order_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|text| Uuid::parse_str(text).ok());
            Ok(Event {
                target_id: row.try_get("watchlist_target_id")?,
                event_type: row.try_get("event_type")?,
                order_id,
                is_read: row.try_get("is_read")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let mut metrics =
        Metrics::from_visible_events(&targets, &events, visible_order_ids, best_price_cutoff);
    let mut counts = HashMap::new();
    for order in visible_orders {
        *counts
            .entry((
                &order.market_product,
                order.delivery_point_id,
                &order.availability_window,
            ))
            .or_insert(0i64) += 1;
    }
    for target in &targets {
        if let (Some(product), Some(point), Some(window)) = (
            &target.market_product_code,
            target.delivery_point_id,
            &target.availability_window_code,
        ) {
            metrics.active.insert(
                target.id,
                *counts.get(&(product, point, window)).unwrap_or(&0),
            );
        }
    }
    Ok(Some(project_summary(
        container.try_get("id")?,
        container.try_get("name")?,
        container.try_get("kind")?,
        container.try_get("created_at")?,
        targets,
        &metrics,
    )))
}

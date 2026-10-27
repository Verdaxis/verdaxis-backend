//! Actor-scoped batched loading. The caller supplies the shared public-order
//! result captured on this transaction; no private visibility policy is copied.
use super::events::source_order_sql;
use super::{Event, Metrics, Summary, Target, project_summary};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct LegacyEntry {
    pub id: Uuid,
    pub product_id: Uuid,
    pub product_name: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub delivery_point_name: Option<String>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct LegacyWatchlist {
    pub id: Uuid,
    pub name: String,
    pub entry_count: usize,
    pub entries: Vec<LegacyEntry>,
    pub created_at: DateTime<Utc>,
}

/// One container query and one joined child query for the actor's full list.
/// Entries have no source relationship ordering and retain their null quotes.
pub async fn load_legacy_list(
    connection: &mut PgConnection,
    actor_id: Uuid,
) -> Result<Vec<LegacyWatchlist>, sqlx::Error> {
    let containers = sqlx::query(
        "SELECT id,name,created_at FROM watchlists WHERE user_id=$1 ORDER BY created_at DESC",
    )
    .bind(actor_id)
    .fetch_all(&mut *connection)
    .await?;
    let rows=sqlx::query("SELECT json_build_object('id',e.id,'product_id',e.product_id,'product_name',p.name,'delivery_point_id',e.delivery_point_id,'delivery_point_name',d.name,'best_bid',NULL,'best_ask',NULL,'created_at',e.created_at)::text AS value,e.watchlist_id FROM watchlist_entries e JOIN watchlists w ON w.id=e.watchlist_id LEFT JOIN products p ON p.id=e.product_id LEFT JOIN delivery_points d ON d.id=e.delivery_point_id WHERE w.user_id=$1")
        .bind(actor_id).fetch_all(&mut *connection).await?;
    let mut entries = HashMap::<Uuid, Vec<LegacyEntry>>::new();
    for row in rows {
        let raw: String = row.try_get("value")?;
        let entry =
            serde_json::from_str(&raw).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        entries
            .entry(row.try_get("watchlist_id")?)
            .or_default()
            .push(entry);
    }
    containers
        .into_iter()
        .map(|row| {
            let id = row.try_get("id")?;
            let children = entries.remove(&id).unwrap_or_default();
            Ok(LegacyWatchlist {
                id,
                name: row.try_get("name")?,
                entry_count: children.len(),
                entries: children,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct VisibleOrder {
    pub market_product: String,
    pub delivery_point_id: Uuid,
    pub availability_window: String,
}

/// One captured public relation supplies both PIN membership and count keys.
/// Route adapters should use `capture_visible` rather than assembling two
/// independent datasets with potentially different clocks or policy states.
pub struct VisibleSnapshot {
    ids: HashSet<Uuid>,
    orders: Vec<VisibleOrder>,
}

impl VisibleSnapshot {
    pub fn from_orders(rows: Vec<(Uuid, String, Uuid, String)>) -> Self {
        let mut ids = HashSet::new();
        let orders = rows
            .into_iter()
            .map(
                |(id, market_product, delivery_point_id, availability_window)| {
                    ids.insert(id);
                    VisibleOrder {
                        market_product,
                        delivery_point_id,
                        availability_window,
                    }
                },
            )
            .collect();
        Self { ids, orders }
    }
    pub fn contains(&self, id: Uuid) -> bool {
        self.ids.contains(&id)
    }
    /// The full public order scope as a bindable list, for reads that filter
    /// rows in SQL.
    pub fn visible_ids(&self) -> Vec<Uuid> {
        self.ids.iter().copied().collect()
    }
    pub fn order_count(&self) -> usize {
        self.orders.len()
    }
    pub async fn detail(
        &self,
        connection: &mut PgConnection,
        actor: Uuid,
        id: Uuid,
        cutoff: DateTime<Utc>,
    ) -> Result<Option<Summary>, sqlx::Error> {
        load_detail(connection, actor, id, &self.ids, &self.orders, cutoff).await
    }
}

pub async fn capture_visible(
    connection: &mut PgConnection,
    policy: &crate::MarketPolicy,
    clock: &crate::market::visibility::VisibilityClock,
    config: &crate::Config,
) -> Result<VisibleSnapshot, sqlx::Error> {
    let rows = super::visible_order_rows(connection, policy, clock, config).await?;
    let values = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get("id")?,
                row.try_get("market_product")?,
                row.try_get("delivery_point_id")?,
                row.try_get("availability_window")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(VisibleSnapshot::from_orders(values))
}

const EVENTS_SQL: &str = concat!(
    "SELECT e.watchlist_target_id,e.event_type::text AS event_type,",
    "e.is_read,e.created_at,",
    source_order_sql!(),
    " FROM watchlist_events e WHERE e.watchlist_id=$1"
);

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
    // The counts read the source order with the feed's own rule.
    let rows = sqlx::query(EVENTS_SQL)
        .bind(watchlist_id)
        .fetch_all(&mut *connection)
        .await?;
    let events = rows
        .iter()
        .map(|row| {
            Ok(Event {
                target_id: row.try_get("watchlist_target_id")?,
                event_type: row.try_get("event_type")?,
                order_id: row.try_get("source_order")?,
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

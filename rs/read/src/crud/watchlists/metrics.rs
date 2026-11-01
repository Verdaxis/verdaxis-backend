//! Container counts over the shared admission relation and public order scope.
use super::admission::{EventAdmission, event_visible_sql};
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default)]
pub struct Metrics {
    pub active: HashMap<Uuid, i64>,
    pub unread: HashMap<Uuid, i64>,
    pub latest: HashMap<Uuid, DateTime<Utc>>,
}

/// Select one public-order set through the Phase 1 relation and binder. Both
/// PIN visibility and slice counts must consume this result, never a local
/// owner/provenance/KYC predicate.
pub async fn visible_order_rows(
    connection: &mut sqlx::PgConnection,
    policy: &crate::MarketPolicy,
    clock: &crate::market::visibility::VisibilityClock,
    config: &crate::Config,
) -> Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> {
    use crate::market::visibility::{PublicOrderFilters, bind_order_query, order_query_sql};
    let query = order_query_sql(
        "SELECT id,market_product,delivery_point_id,availability_window FROM eligible_orders",
    );
    bind_order_query(
        &query,
        policy,
        clock,
        config,
        &PublicOrderFilters::default(),
    )?
    .fetch_all(connection)
    .await
}

// Admission runs in `WHERE`, before aggregation, so a refused row moves
// neither a count nor a latest time. The inner join keeps the feed's target
// rule: a row whose target is gone has no target to count toward.
const EVENT_METRICS_SQL: &str = concat!(
    "SELECT e.watchlist_target_id,",
    "count(*) FILTER (WHERE NOT e.is_read) AS unread,",
    "max(e.created_at) AS latest ",
    "FROM watchlist_events e ",
    "JOIN watchlist_targets t ON t.id=e.watchlist_target_id ",
    "WHERE e.watchlist_id=$1 AND ",
    event_visible_sql!("$2", "$3"),
    " GROUP BY e.watchlist_target_id"
);

impl Metrics {
    /// Unread count and latest event time per target, over admitted rows only.
    /// Active-order counts are filled by the caller from the same captured scope.
    pub(super) async fn load_events(
        connection: &mut sqlx::PgConnection,
        watchlist: Uuid,
        visible_order_ids: Vec<Uuid>,
        admission: EventAdmission,
    ) -> Result<Self, sqlx::Error> {
        let rows = sqlx::query(EVENT_METRICS_SQL)
            .bind(watchlist)
            .bind(visible_order_ids)
            .bind(admission.activation())
            .fetch_all(connection)
            .await?;
        let mut metrics = Self::default();
        for row in rows {
            let target: Uuid = row.try_get("watchlist_target_id")?;
            metrics.unread.insert(target, row.try_get("unread")?);
            metrics.latest.insert(target, row.try_get("latest")?);
        }
        Ok(metrics)
    }
}

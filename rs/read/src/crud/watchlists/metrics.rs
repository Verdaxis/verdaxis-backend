//! Metrics consume the shared public visibility result, never a second policy.
use super::Target;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub struct Event {
    pub target_id: Uuid,
    pub event_type: String,
    pub order_id: Option<Uuid>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

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

impl Metrics {
    /// PD2/PD4-G: filter before counting/latest aggregation; retain every row.
    /// Current rows have no trusted producer attestation. Best-price event type
    /// is therefore a durable deny rule, regardless of source ID or timestamp.
    /// The explicit cutoff is reserved for the later trusted producer (PD5).
    pub fn from_visible_events(
        targets: &[Target],
        events: &[Event],
        visible_orders: &HashSet<Uuid>,
        _best_price_cutoff: DateTime<Utc>,
    ) -> Self {
        let allowed = targets
            .iter()
            .filter(|target| {
                target.target_type == "SLICE"
                    || target
                        .order_id
                        .is_some_and(|id| visible_orders.contains(&id))
            })
            .map(|target| target.id)
            .collect::<HashSet<_>>();
        let mut result = Self::default();
        for event in events {
            if !allowed.contains(&event.target_id)
                || event
                    .order_id
                    .is_some_and(|id| !visible_orders.contains(&id))
                || event.event_type == "SLICE_BEST_PRICE_MOVED"
            {
                continue;
            }
            if !event.is_read {
                *result.unread.entry(event.target_id).or_default() += 1;
            }
            result
                .latest
                .entry(event.target_id)
                .and_modify(|value| *value = (*value).max(event.created_at))
                .or_insert(event.created_at);
        }
        result
    }
}

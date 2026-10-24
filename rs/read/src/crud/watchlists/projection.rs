//! Pure watchlist DTO projection. Database loading and mutation are separate.
use super::Metrics;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: Uuid,
    pub target_type: String,
    pub market_product_code: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub delivery_point_name: Option<String>,
    pub availability_window_code: Option<String>,
    pub order_id: Option<Uuid>,
    pub snapshot_price_per_mt_usd: Option<f64>,
    pub snapshot_quantity_mt: Option<f64>,
    pub snapshot_remaining_quantity_mt: Option<f64>,
    pub snapshot_status: Option<String>,
    pub snapshot_side: Option<String>,
    pub snapshot_market_product: Option<String>,
    pub snapshot_delivery_point_name: Option<String>,
    pub snapshot_availability_window: Option<String>,
    pub snapshot_counterparty_label: Option<String>,
    #[serde(default)]
    pub active_order_count: i64,
    #[serde(default)]
    pub unread_event_count: i64,
    pub latest_event_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct Slice {
    pub id: Uuid,
    pub target_type: &'static str,
    pub market_product_code: String,
    pub delivery_point_id: Uuid,
    pub delivery_point_name: Option<String>,
    pub availability_window_code: String,
    pub active_order_count: i64,
    pub unread_event_count: i64,
    pub latest_event_at: Option<DateTime<Utc>>,
    pub pins: Vec<Target>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub unread_event_count: i64,
    pub latest_event_at: Option<DateTime<Utc>>,
    pub total_slice_count: usize,
    pub has_more_slices: bool,
    pub slices: Vec<Slice>,
    pub created_at: DateTime<Utc>,
}

type SliceKey = (String, Uuid, String);
fn key(target: &Target) -> Option<SliceKey> {
    Some((
        target.market_product_code.clone()?,
        target.delivery_point_id?,
        target.availability_window_code.clone()?,
    ))
}

pub fn project_summary(
    id: Uuid,
    name: String,
    kind: String,
    created_at: DateTime<Utc>,
    mut targets: Vec<Target>,
    metrics: &Metrics,
) -> Summary {
    targets.sort_by_key(|target| std::cmp::Reverse(target.created_at));
    let mut pins = std::collections::HashMap::<SliceKey, Vec<Target>>::new();
    for target in targets.iter().filter(|target| target.target_type == "PIN") {
        if let Some(key) = key(target) {
            let mut pin = target.clone();
            pin.active_order_count = 0;
            pin.unread_event_count = *metrics.unread.get(&pin.id).unwrap_or(&0);
            pin.latest_event_at = metrics.latest.get(&pin.id).copied();
            pins.entry(key).or_default().push(pin);
        }
    }
    for group in pins.values_mut() {
        group.sort_by_key(|pin| std::cmp::Reverse((pin.latest_event_at, pin.created_at)));
    }
    let mut slices = Vec::new();
    for target in targets
        .iter()
        .filter(|target| target.target_type == "SLICE")
    {
        let Some((product, point, window)) = key(target) else {
            continue;
        };
        let group = pins
            .remove(&(product.clone(), point, window.clone()))
            .unwrap_or_default();
        let latest = metrics
            .latest
            .get(&target.id)
            .copied()
            .into_iter()
            .chain(group.iter().filter_map(|pin| pin.latest_event_at))
            .max();
        slices.push(Slice {
            id: target.id,
            target_type: "SLICE",
            market_product_code: product,
            delivery_point_id: point,
            delivery_point_name: target
                .delivery_point_name
                .clone()
                .or_else(|| target.snapshot_delivery_point_name.clone()),
            availability_window_code: window,
            active_order_count: *metrics.active.get(&target.id).unwrap_or(&0),
            unread_event_count: metrics.unread.get(&target.id).copied().unwrap_or(0)
                + group.iter().map(|pin| pin.unread_event_count).sum::<i64>(),
            latest_event_at: latest,
            pins: group,
            created_at: target.created_at,
        });
    }
    slices.sort_by_key(|slice| {
        std::cmp::Reverse((
            slice.unread_event_count > 0,
            slice.latest_event_at,
            slice.created_at,
        ))
    });
    let total_slice_count = slices.len();
    let unread_event_count = slices.iter().map(|slice| slice.unread_event_count).sum();
    let latest_event_at = slices
        .iter()
        .filter_map(|slice| slice.latest_event_at)
        .max();
    slices.truncate(100);
    Summary {
        id,
        name,
        kind,
        unread_event_count,
        latest_event_at,
        total_slice_count,
        has_more_slices: total_slice_count > 100,
        slices,
        created_at,
    }
}

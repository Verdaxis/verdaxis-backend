//! Slice liquidity for one provenance source.
//!
//! Liquidity is a marketplace diagnostic computed from currently eligible
//! quotes, grouped into exact `(product, delivery point, window)` slices. It is
//! never merged across provenance: demo depth stays in the demo section and
//! unknown-provenance quotes contribute to no slice at all.
//!
//! A slice publishes price, spread and depth only when at least three distinct
//! organizations contribute. Below that the price-derived values stay private:
//! the row is still listed (its existence is not a cell) with
//! `suppressed = true` and no values.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, TimeDelta, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use uuid::Uuid;

use super::marketplace::{
    derive_product_code, money, rate_pct, slugify, window_label, wire, Quality, QuoteRow,
    MAX_RANKED_ROWS, SUPPRESSION_THRESHOLD,
};
use crate::MarketPolicy;

#[derive(Clone, Debug, Serialize)]
pub struct SliceLiquidityRow {
    pub product_key: String,
    pub product_label: String,
    pub delivery_point_key: String,
    pub delivery_point_label: String,
    pub availability_window: String,
    pub availability_window_label: String,
    pub contributing_organizations: Option<i64>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub best_bid_usd_per_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub best_ask_usd_per_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub spread_usd_per_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub spread_bps: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub best_bid_depth_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub best_ask_depth_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub one_percent_bid_depth_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub one_percent_ask_depth_mt: Option<Decimal>,
    pub crossed: Option<bool>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LiquiditySummary {
    pub two_sided_slices: Option<i64>,
    pub one_sided_slices: Option<i64>,
    pub crossed_slices: Option<i64>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub median_spread_usd_per_mt: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub median_spread_bps: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub median_open_order_age_hours: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub median_hours_to_first_fill: Option<Decimal>,
    pub slices: Vec<SliceLiquidityRow>,
}

#[derive(Default)]
struct SliceEntry {
    product_label: String,
    delivery_point_label: String,
    organizations: BTreeSet<Uuid>,
    bids: Vec<(Decimal, Decimal)>,
    asks: Vec<(Decimal, Decimal)>,
    created: Vec<DateTime<Utc>>,
}

/// Median with Python's `statistics.median` semantics: the middle value for an
/// odd count, the mean of the two middle values for an even count.
fn median(values: &mut [Decimal]) -> Option<Decimal> {
    if values.is_empty() {
        return None;
    }
    values.sort();
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / Decimal::TWO
    } else {
        values[middle]
    })
}

fn depth(quotes: &[(Decimal, Decimal)], predicate: impl Fn(Decimal) -> bool) -> Decimal {
    quotes
        .iter()
        .filter(|(price, _)| predicate(*price))
        .map(|(_, quantity)| *quantity)
        .sum()
}

/// Compose the liquidity summary for one provenance label.
pub fn summarize(
    policy: &MarketPolicy,
    provenance: &str,
    quotes: &[QuoteRow],
    fill_rows: &[super::marketplace::FillRow],
    as_of: DateTime<Utc>,
    quality: &mut Quality,
) -> LiquiditySummary {
    let mut slices: BTreeMap<(String, String, String), SliceEntry> = BTreeMap::new();
    for row in quotes {
        // Unknown provenance never contributes liquidity, and a section only
        // ever sees its own provenance.
        if row.provenance != provenance || provenance == "unknown" {
            continue;
        }
        let product_key =
            derive_product_code(policy, &row.product_name, &row.fuel_type, &row.fuel_grade)
                .map(str::to_owned)
                .unwrap_or_else(|| slugify(&row.product_name));
        let key = (
            product_key,
            slugify(&row.delivery_point_name),
            row.window.clone(),
        );
        let entry = slices.entry(key).or_insert_with(|| SliceEntry {
            product_label: row.product_name.clone(),
            delivery_point_label: row.delivery_point_name.clone(),
            ..SliceEntry::default()
        });
        entry.organizations.insert(row.organization_id);
        entry.created.push(row.created_at);
        if row.side == "BID" {
            entry.bids.push((row.price, row.remaining));
        } else {
            entry.asks.push((row.price, row.remaining));
        }
    }

    let mut rows: Vec<SliceLiquidityRow> = Vec::new();
    let mut two_sided: i64 = 0;
    let mut one_sided: i64 = 0;
    let mut crossed: i64 = 0;
    let mut spreads_usd: Vec<Decimal> = Vec::new();
    let mut spreads_bps: Vec<Decimal> = Vec::new();
    let mut ages: Vec<TimeDelta> = Vec::new();

    for ((product_key, delivery_point_key, window), entry) in slices {
        let bids = &entry.bids;
        let asks = &entry.asks;
        if !bids.is_empty() && !asks.is_empty() {
            two_sided += 1;
        } else if !bids.is_empty() || !asks.is_empty() {
            one_sided += 1;
        }
        ages.extend(entry.created.iter().map(|created| as_of - *created));

        let organization_count = entry.organizations.len() as i64;
        let suppressed = organization_count < SUPPRESSION_THRESHOLD;
        let best_bid = bids.iter().map(|(price, _)| *price).max();
        let best_ask = asks.iter().map(|(price, _)| *price).min();

        let mut spread: Option<Decimal> = None;
        let mut spread_bps: Option<Decimal> = None;
        let mut is_crossed: Option<bool> = None;
        if let (Some(best_bid_price), Some(best_ask_price)) = (best_bid, best_ask) {
            let value = best_ask_price - best_bid_price;
            spread = Some(value);
            let mid = (best_ask_price + best_bid_price) / Decimal::TWO;
            if mid > Decimal::ZERO {
                let mut bps = (value / mid * Decimal::from(10_000))
                    .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
                bps.rescale(2);
                spread_bps = Some(bps);
            }
            is_crossed = Some(value < Decimal::ZERO);
            if value < Decimal::ZERO {
                crossed += 1;
            }
            if !suppressed {
                spreads_usd.push(value);
                if let Some(bps) = spread_bps {
                    spreads_bps.push(bps);
                }
            }
        }

        if suppressed {
            quality.suppressed_cell_count += 1;
            rows.push(SliceLiquidityRow {
                product_key,
                product_label: entry.product_label,
                delivery_point_key,
                delivery_point_label: entry.delivery_point_label,
                availability_window_label: window_label(&window),
                availability_window: window,
                contributing_organizations: None,
                best_bid_usd_per_mt: None,
                best_ask_usd_per_mt: None,
                spread_usd_per_mt: None,
                spread_bps: None,
                best_bid_depth_mt: None,
                best_ask_depth_mt: None,
                one_percent_bid_depth_mt: None,
                one_percent_ask_depth_mt: None,
                crossed: None,
                suppressed: true,
            });
            continue;
        }

        // A zero best price carries no depth information in the Python
        // reference (`if best_bid`), so its depth stays absent.
        let bid_reference = best_bid.filter(|price| !price.is_zero());
        let ask_reference = best_ask.filter(|price| !price.is_zero());
        let one_percent_bid = bid_reference.map(|reference| {
            let floor = reference * Decimal::new(99, 2);
            depth(bids, |price| price >= floor)
        });
        let one_percent_ask = ask_reference.map(|reference| {
            let ceiling = reference * Decimal::new(101, 2);
            depth(asks, |price| price <= ceiling)
        });
        rows.push(SliceLiquidityRow {
            product_key,
            product_label: entry.product_label,
            contributing_organizations: Some(organization_count),
            delivery_point_key,
            delivery_point_label: entry.delivery_point_label,
            availability_window_label: window_label(&window),
            availability_window: window,
            best_bid_usd_per_mt: best_bid,
            best_ask_usd_per_mt: best_ask,
            spread_usd_per_mt: spread,
            spread_bps,
            best_bid_depth_mt: bid_reference
                .map(|reference| depth(bids, |price| price == reference)),
            best_ask_depth_mt: ask_reference
                .map(|reference| depth(asks, |price| price == reference)),
            one_percent_bid_depth_mt: one_percent_bid,
            one_percent_ask_depth_mt: one_percent_ask,
            crossed: is_crossed,
            suppressed: false,
        });
    }

    let fills: Vec<TimeDelta> = fill_rows
        .iter()
        .filter(|row| row.first_confirmed_at >= row.created_at)
        .map(|row| row.first_confirmed_at - row.created_at)
        .collect();

    rows.truncate(MAX_RANKED_ROWS);
    LiquiditySummary {
        two_sided_slices: Some(two_sided),
        one_sided_slices: Some(one_sided),
        crossed_slices: Some(crossed),
        median_spread_usd_per_mt: median(&mut spreads_usd).map(money),
        median_spread_bps: median(&mut spreads_bps).map(money),
        median_open_order_age_hours: super::marketplace::median_hours(&ages),
        median_hours_to_first_fill: if provenance == "live" {
            super::marketplace::median_hours(&fills)
        } else {
            None
        },
        slices: rows,
    }
}

/// Share of a slice total, kept here so the two callers agree on the rounding.
pub fn share(count: i64, total: i64) -> Option<Decimal> {
    rate_pct(count, total)
}

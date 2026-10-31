use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use super::invalidation::{InvalidationError, refusal};
use crate::{
    Config, MarketPolicy,
    market::{
        live_benchmarks::{LiveBenchmarkKey, calculate_live_benchmarks},
        windows::is_canonical_window,
    },
};

#[derive(Clone, Debug)]
pub struct BenchmarkRebuildKey {
    pub side: String,
    pub market_product: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: Option<String>,
}

#[derive(Debug, Default)]
pub struct BenchmarkRebuildResult {
    pub rebuilt: Vec<LiveBenchmarkKey>,
    pub deleted: Vec<LiveBenchmarkKey>,
    pub ignored_null_points: usize,
}

pub(super) fn normalize_keys(
    policy: &MarketPolicy,
    keys: &[BenchmarkRebuildKey],
) -> Result<(Vec<LiveBenchmarkKey>, usize), InvalidationError> {
    let mut unique = BTreeMap::new();
    let mut ignored = 0;
    for key in keys {
        let Some(point) = key.delivery_point_id else {
            ignored += 1;
            continue;
        };
        let (Some(product), Some(window)) = (&key.market_product, &key.availability_window) else {
            continue;
        };
        if product.is_empty() || window.is_empty() {
            continue;
        }
        if !matches!(key.side.as_str(), "BID" | "ASK")
            || !is_canonical_window(window)
            || !policy.products.iter().any(|p| p.code == *product)
        {
            return Err(refusal(
                "BENCHMARK_KEY_INVALID",
                "The benchmark key is not in the supplied market policy.",
            ));
        }
        unique.insert(
            (key.side.clone(), product.clone(), point, window.clone()),
            LiveBenchmarkKey {
                side: key.side.clone(),
                product: product.clone(),
                point,
                window: window.clone(),
            },
        );
        if unique.len() > 100 {
            return Err(refusal(
                "MARKET_INVALIDATION_LIMIT",
                "The benchmark key bound was exceeded.",
            ));
        }
    }
    Ok((unique.into_values().collect(), ignored))
}

pub async fn rebuild_live_slice_benchmarks_for_keys(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    config: &Config,
    keys: &[BenchmarkRebuildKey],
    now: DateTime<Utc>,
) -> Result<BenchmarkRebuildResult, InvalidationError> {
    let (keys, ignored_null_points) = normalize_keys(policy, keys)?;
    let mut result = BenchmarkRebuildResult {
        ignored_null_points,
        ..Default::default()
    };
    if keys.is_empty() {
        return Ok(result);
    }
    let calculations = calculate_live_benchmarks(conn, policy, config, &keys, now).await?;
    for key in keys {
        if let Some(value) = calculations.get(&key) {
            let count = i32::try_from(value.count).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
            sqlx::query("INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'live_slice_vwap',$9,$9) ON CONFLICT ON CONSTRAINT uq_live_slice_benchmarks_slice_key DO UPDATE SET benchmark_price_per_mt_usd=EXCLUDED.benchmark_price_per_mt_usd,total_remaining_quantity_mt=EXCLUDED.total_remaining_quantity_mt,order_count=EXCLUDED.order_count,source=EXCLUDED.source,updated_at=EXCLUDED.updated_at")
                .bind(Uuid::new_v4()).bind(&key.side).bind(&key.product).bind(key.point).bind(&key.window).bind(value.price).bind(value.quantity).bind(count).bind(now).execute(&mut *conn).await?;
            result.rebuilt.push(key);
        } else {
            sqlx::query("DELETE FROM live_slice_benchmarks WHERE side=$1 AND market_product=$2 AND delivery_point_id=$3 AND availability_window=$4")
                .bind(&key.side).bind(&key.product).bind(key.point).bind(&key.window).execute(&mut *conn).await?;
            result.deleted.push(key);
        }
    }
    Ok(result)
}

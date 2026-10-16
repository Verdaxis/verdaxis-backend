//! Read-only, same-side live benchmarks. REAL and DEMO intentionally share keys.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::{ApiError, Config, Json, MarketPolicy};

use super::terms::{QualificationInput, order_is_execution_qualified_at};
use super::visibility::{
    OrderSurface, PublicOrderFilters, VisibilityClock, benchmark_candidate_sql, bind_order_query,
};
use super::whitespace::is_python_whitespace;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LiveBenchmarkKey {
    pub side: String,
    pub product: String,
    pub point: Uuid,
    pub window: String,
}

#[derive(Clone, Debug)]
pub struct LiveBenchmark {
    pub price: Decimal,
    pub quantity: Decimal,
    pub count: u64,
}

fn cents(value: Decimal) -> Decimal {
    let mut value = value.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    value.rescale(2);
    value
}

pub fn qualified_vwap(rows: &[(Decimal, Decimal)]) -> Option<Decimal> {
    let quantity: Decimal = rows.iter().map(|(quantity, _)| *quantity).sum();
    if quantity <= Decimal::ZERO {
        return None;
    }
    let weighted: Decimal = rows.iter().map(|(quantity, price)| quantity * price).sum();
    Some(cents(weighted / quantity))
}

/// Additional benchmark qualification after the Candidates relation. Catalog,
/// lane, status, expiry, provenance and owner admission belong to that relation.
/// This internal row must never be serialized as a public response.
pub fn candidate_is_benchmark_qualified(
    policy: &MarketPolicy,
    row: &Json,
    clock: &VisibilityClock,
) -> bool {
    let text = |name| row.get(name).and_then(Json::as_str);
    let flag = |name| row.get(name).and_then(Json::as_bool).unwrap_or(false);
    let Some(product_id) = text("product_id").and_then(|value| value.parse::<Uuid>().ok()) else {
        return false;
    };
    let Some(product) = policy
        .products
        .iter()
        .find(|product| product.id == product_id)
    else {
        return false;
    };
    let positive = text("remaining_quantity_mt")
        .and_then(|value| value.parse::<Decimal>().ok())
        .is_some_and(|quantity| quantity > Decimal::ZERO);
    if !positive || flag("off_spec") {
        return false;
    }
    let side = text("side").unwrap_or_default();
    let input = QualificationInput {
        product_id: Some(product_id),
        market_product: Some(&product.code),
        side,
        delivery_point_id: text("delivery_point_id"),
        availability_window: text("availability_window").unwrap_or_default(),
        off_spec: flag("off_spec"),
        msds_available: flag("msds_available"),
        certification_scheme: text("certification_scheme"),
        certification_declared: flag("certification_declared"),
        fame_terms: row.get("fame_terms"),
        lng_terms: row.get("lng_terms"),
        specification_standard: text("specification_standard"),
        carbon_intensity_method: text("carbon_intensity_method"),
        carbon_intensity_gco2_mj: row.get("carbon_intensity_gco2_mj").and_then(Json::as_str),
        feedstock: text("feedstock"),
        origin: text("origin"),
    };
    let typed = matches!(product.qualification_profile.as_str(), "fame_v1" | "lng_v1");
    if typed {
        return order_is_execution_qualified_at(policy, &input, clock.singapore_date);
    }
    if side != "ASK" {
        return true;
    }
    order_is_execution_qualified_at(policy, &input, clock.singapore_date)
        && flag("msds_available")
        && row
            .get("carbon_intensity_gco2_mj")
            .is_some_and(|value| !value.is_null())
        && ["specification_standard", "feedstock", "origin"]
            .iter()
            .all(|name| {
                text(name).is_some_and(|value| !value.trim_matches(is_python_whitespace).is_empty())
            })
}

/// One batched SELECT on the caller's admitted transaction and captured clock.
/// Missing keys are absent, just like Python's calculation map. Use
/// `LiveBenchmarkCache` when more than one batch is needed within a request.
pub async fn live_benchmarks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy: &MarketPolicy,
    config: &Config,
    keys: &[LiveBenchmarkKey],
    now: DateTime<Utc>,
) -> Result<HashMap<LiveBenchmarkKey, LiveBenchmark>, ApiError> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let clock = VisibilityClock::at(now);
    let sql = benchmark_candidate_sql(include_str!("../../sql/live_benchmarks.sql"));
    let filters = PublicOrderFilters {
        surface: OrderSurface::Candidates,
        ..Default::default()
    };
    let result: Result<_, sqlx::Error> = async {
        let keys_json =
            serde_json::to_string(keys).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        // Admission subplans make JIT compilation cost more than this bounded
        // read. Restore the caller's setting after the statement; rollback
        // restores it automatically if the statement fails.
        let previous_jit: String = sqlx::query_scalar("SHOW jit").fetch_one(&mut **tx).await?;
        sqlx::query("SET LOCAL jit = off")
            .execute(&mut **tx)
            .await?;
        let rows = bind_order_query(&sql, policy, &clock, config, &filters)?
            .bind(keys_json)
            .fetch_all(&mut **tx)
            .await?;
        sqlx::query("SELECT set_config('jit', $1, true)")
            .bind(previous_jit)
            .execute(&mut **tx)
            .await?;
        let mut groups: HashMap<LiveBenchmarkKey, Vec<(Decimal, Decimal)>> = HashMap::new();
        for row in rows {
            let input: Json = row.try_get("private_digest_input")?;
            if !candidate_is_benchmark_qualified(policy, &input, &clock) {
                continue;
            }
            let key = LiveBenchmarkKey {
                side: row.try_get("side")?,
                product: row.try_get("market_product")?,
                point: row.try_get("delivery_point_id")?,
                window: row.try_get("availability_window")?,
            };
            groups.entry(key).or_default().push((
                row.try_get("remaining_quantity_mt")?,
                row.try_get("price_per_mt_usd")?,
            ));
        }
        Ok(groups
            .into_iter()
            .filter_map(|(key, rows)| {
                qualified_vwap(&rows).map(|price| {
                    (
                        key,
                        LiveBenchmark {
                            price,
                            quantity: cents(rows.iter().map(|(quantity, _)| *quantity).sum()),
                            count: rows.len() as u64,
                        },
                    )
                })
            })
            .collect())
    }
    .await;
    result.map_err(|error| {
        crate::runtime::error::database_error(
            "/api/orderbook",
            "GET",
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
        )
    })
}

/// Request-local cache, including missing results. Never share across requests
/// or change the transaction, configuration, policy or instant within a cache.
#[derive(Default)]
pub struct LiveBenchmarkCache {
    values: HashMap<LiveBenchmarkKey, Option<LiveBenchmark>>,
}

impl LiveBenchmarkCache {
    pub async fn load(
        &mut self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        policy: &MarketPolicy,
        config: &Config,
        keys: &[LiveBenchmarkKey],
        now: DateTime<Utc>,
    ) -> Result<HashMap<LiveBenchmarkKey, Option<LiveBenchmark>>, ApiError> {
        let missing: Vec<_> = keys
            .iter()
            .filter(|key| !self.values.contains_key(*key))
            .cloned()
            .collect();
        let mut loaded = live_benchmarks(tx, policy, config, &missing, now).await?;
        for key in missing {
            self.values
                .entry(key.clone())
                .or_insert_with(|| loaded.remove(&key));
        }
        Ok(keys
            .iter()
            .map(|key| (key.clone(), self.values[key].clone()))
            .collect())
    }
}

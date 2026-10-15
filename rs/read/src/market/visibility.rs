//! Shared public visibility: the bound SQL and its parameter binding.
//!
//! The statements live in `rs/read/sql/` and are returned whole by
//! [`public_order_sql`] and [`trade_evidence_sql`]; callers bind parameters
//! and never build SQL text. Route code runs them through
//! [`select_public_order_ids`] and [`select_trade_evidence_ids`] inside
//! `runtime::admission::run_admitted`, so the permit covers the database work.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::MarketPolicy;

/// The complete bound public-order SELECT.
pub fn public_order_sql() -> &'static str {
    include_str!("../../sql/public_orders.sql")
}

/// The complete bound formal-trade-evidence SELECT.
pub fn trade_evidence_sql() -> &'static str {
    include_str!("../../sql/trade_evidence.sql")
}

/// The instant a visibility decision is made at, plus the Singapore calendar
/// date the stored declaration rules compare against.
#[derive(Clone, Copy, Debug)]
pub struct VisibilityClock {
    pub now: DateTime<Utc>,
    pub singapore_date: NaiveDate,
}

impl VisibilityClock {
    /// Derive the Singapore date from one captured instant.
    pub fn at(now: DateTime<Utc>) -> Self {
        Self {
            now,
            singapore_date: now.with_timezone(&chrono_tz::Asia::Singapore).date_naive(),
        }
    }
}

/// Route filters for the shared public-order statement. Every field is bound;
/// `None` means "no filter". `limit` must be positive.
#[derive(Clone, Debug)]
pub struct PublicOrderFilters<'a> {
    pub side: Option<&'a str>,
    pub product_id: Option<Uuid>,
    pub delivery_point_id: Option<Uuid>,
    pub fuel_type: Option<&'a str>,
    pub market_product: Option<&'a str>,
    pub region: Option<&'a str>,
    pub availability_window: Option<&'a str>,
    pub include_off_spec: bool,
    pub skip: i64,
    pub limit: i64,
}

impl Default for PublicOrderFilters<'_> {
    fn default() -> Self {
        Self {
            side: None,
            product_id: None,
            delivery_point_id: None,
            fuel_type: None,
            market_product: None,
            region: None,
            availability_window: None,
            include_off_spec: false,
            skip: 0,
            limit: i64::MAX,
        }
    }
}

fn policy_json(policy: &MarketPolicy) -> Result<String, sqlx::Error> {
    serde_json::to_string(policy).map_err(|error| sqlx::Error::Encode(Box::new(error)))
}

/// Select the ids of every publicly visible order matching `filters`.
pub async fn select_public_order_ids(
    connection: &mut sqlx::PgConnection,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    filters: &PublicOrderFilters<'_>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let policy = policy_json(policy)?;
    let singapore_date = clock.singapore_date.format("%Y-%m-%d").to_string();
    sqlx::query_scalar::<_, Uuid>(public_order_sql())
        .bind(policy)
        .bind(clock.now)
        .bind(singapore_date)
        .bind(filters.include_off_spec)
        .bind(filters.product_id)
        .bind(filters.delivery_point_id)
        .bind(filters.side)
        .bind(filters.fuel_type)
        .bind(filters.market_product)
        .bind(filters.region)
        .bind(filters.availability_window)
        .bind(filters.limit)
        .bind(filters.skip)
        .fetch_all(connection)
        .await
}

/// Route filters for the shared trade-evidence statement.
#[derive(Clone, Debug)]
pub struct TradeEvidenceFilters<'a> {
    pub confirmed_since: Option<DateTime<Utc>>,
    pub confirmed_before: Option<DateTime<Utc>>,
    pub product_id: Option<Uuid>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: Option<&'a str>,
    pub skip: i64,
    pub limit: i64,
}

impl Default for TradeEvidenceFilters<'_> {
    fn default() -> Self {
        Self {
            confirmed_since: None,
            confirmed_before: None,
            product_id: None,
            delivery_point_id: None,
            availability_window: None,
            skip: 0,
            limit: i64::MAX,
        }
    }
}

/// Select the ids of every formal trade snapshot matching `filters`.
pub async fn select_trade_evidence_ids(
    connection: &mut sqlx::PgConnection,
    policy: &MarketPolicy,
    filters: &TradeEvidenceFilters<'_>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let policy = policy_json(policy)?;
    sqlx::query_scalar::<_, Uuid>(trade_evidence_sql())
        .bind(policy)
        .bind(filters.confirmed_since)
        .bind(filters.confirmed_before)
        .bind(filters.product_id)
        .bind(filters.delivery_point_id)
        .bind(filters.availability_window)
        .bind(filters.limit)
        .bind(filters.skip)
        .fetch_all(connection)
        .await
}

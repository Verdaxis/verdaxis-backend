//! Shared public visibility: the bound SQL and its parameter binding.
//!
//! The SQL files are unpaged internal relations. Sealed query constructors
//! compose developer-owned static tails; bind helpers supply request values.
//! Execute the query inside `runtime::admission::run_admitted` on its connection.
//! Private digest input is internal only and must never become a public DTO.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::MarketPolicy;
use sqlx::{postgres::PgArguments, query::Query};

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

/// Surface policy. Candidates retain only shared current/catalog/provenance rules;
/// benchmarks and curves apply their own typed/scalar qualification afterwards.
#[derive(Clone, Copy, Debug, Default)]
pub enum OrderSurface {
    #[default]
    Orderbook,
    Listings,
    Candidates,
}
impl OrderSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Orderbook => "orderbook",
            Self::Listings => "listings",
            Self::Candidates => "candidates",
        }
    }
}

/// Compose only developer-owned static SQL around the shared relation. The tail
/// reads `eligible_orders` and owns projection, extra filters, ranking and limits.
/// Values remain SQLx binds; no request text is accepted here.
pub struct OrderQuery(String);
pub struct TradeQuery(String);

pub fn order_query_sql(tail: &'static str) -> OrderQuery {
    OrderQuery(format!(
        "WITH eligible_orders AS ({}) {}",
        public_order_sql(),
        tail
    ))
}

/// Same composition boundary for immutable historical evidence.
pub fn trade_query_sql(tail: &'static str) -> TradeQuery {
    TradeQuery(format!(
        "WITH eligible_trades AS ({}) {}",
        trade_evidence_sql(),
        tail
    ))
}

pub fn bind_order_query<'q>(
    sql: &OrderQuery,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    filters: &PublicOrderFilters<'_>,
) -> Result<Query<'q, sqlx::Postgres, PgArguments>, sqlx::Error> {
    Ok(sqlx::query(sqlx::AssertSqlSafe(sql.0.clone()))
        .bind(policy_json(policy)?)
        .bind(clock.now)
        .bind(clock.singapore_date.format("%Y-%m-%d").to_string())
        .bind(filters.include_off_spec)
        .bind(filters.product_id)
        .bind(filters.delivery_point_id)
        .bind(filters.side.map(str::to_owned))
        .bind(filters.fuel_type.map(str::to_owned))
        .bind(filters.market_product.map(str::to_owned))
        .bind(filters.region.map(str::to_owned))
        .bind(filters.availability_window.map(str::to_owned))
        .bind(filters.surface.as_str()))
}

pub fn bind_trade_query<'q>(
    sql: &TradeQuery,
    policy: &MarketPolicy,
    filters: &TradeEvidenceFilters<'_>,
) -> Result<Query<'q, sqlx::Postgres, PgArguments>, sqlx::Error> {
    Ok(sqlx::query(sqlx::AssertSqlSafe(sql.0.clone()))
        .bind(policy_json(policy)?)
        .bind(filters.confirmed_since)
        .bind(filters.confirmed_before)
        .bind(filters.product_id)
        .bind(filters.delivery_point_id)
        .bind(filters.availability_window.map(str::to_owned)))
}

/// Route filters for the shared public-order statement. Every field is bound;
/// `None` means "no filter". `limit` must be positive.
#[derive(Clone, Debug)]
pub struct PublicOrderFilters<'a> {
    pub surface: OrderSurface,
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
            surface: OrderSurface::Orderbook,
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
    let sql = order_query_sql("SELECT id FROM eligible_orders ORDER BY id LIMIT $13 OFFSET $14");
    let rows = bind_order_query(&sql, policy, clock, filters)?
        .bind(filters.limit)
        .bind(filters.skip)
        .fetch_all(connection)
        .await?;
    use sqlx::Row;
    rows.iter().map(|row| row.try_get("id")).collect()
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
    let sql = trade_query_sql("SELECT id FROM eligible_trades ORDER BY id LIMIT $7 OFFSET $8");
    let rows = bind_trade_query(&sql, policy, filters)?
        .bind(filters.limit)
        .bind(filters.skip)
        .fetch_all(connection)
        .await?;
    use sqlx::Row;
    rows.iter().map(|row| row.try_get("id")).collect()
}

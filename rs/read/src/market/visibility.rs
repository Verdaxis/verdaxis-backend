//! Shared public visibility: the bound SQL and its parameter binding.
//!
//! The SQL files are unpaged internal relations. Sealed query constructors
//! compose developer-owned static tails; bind helpers supply request values.
//! Execute the query inside `runtime::admission::run_admitted` on its connection.
//! Private digest input is internal only and must never become a public DTO.

#[path = "price_quarantine.rs"]
pub mod price_quarantine;

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::{Config, MarketPolicy};
use sqlx::{postgres::PgArguments, query::Query};

/// Rust decision 25: the one shared eligibility-set definition, rendered for a
/// consumer's gate bind (`$13` for the shared binder, `$5` for the curve
/// metadata query). Every consumer of `current_order_rule.sql` composes it.
pub fn eligibility_sets_sql(gate: &str) -> String {
    include_str!("../../sql/eligibility_sets.sql").replace("/*gate*/", gate)
}

/// Expand the shared markers a tail or relation may carry. One composition
/// point keeps every consumer's definition identical.
pub fn expand_shared_markers(sql: &str, gate: &str) -> String {
    sql.replace("/* eligibility_sets */", &eligibility_sets_sql(gate))
}

/// Render the shared relation over a chosen base order scan. The default base is
/// the `orderbook_orders` table; the benchmark substitutes a key-filtered scan
/// (decision 27). Every other predicate is identical in both cases.
/// The columns the orderbook map, count and distinct tails read from the
/// shared relation. Declared here, beside the relation it projects, so a
/// column a tail starts reading must be added in this one place.
pub const MAP_PROJECTION: &str = "o.id, o.product_id, o.delivery_point_id, o.side, o.provenance, o.availability_window, o.price_per_mt_usd, o.remaining_quantity_mt, o.created_at, o.updated_at, o.off_spec, o.idempotency_operation,";

/// The count and distinct tails read only the projected labels: no order
/// column at all, so the base rows travel through the predicate and nothing
/// wider. (count groups by market_product; distinct takes the three labels.)
pub const COUNT_PROJECTION: &str = "";
pub const DISTINCT_PROJECTION: &str = "";

/// The projection every consumer needs: the order's own columns plus the
/// catalog labels. `o.*` is the historical default; aggregate and distinct
/// tails replace it through the same marker with only the columns they read,
/// so the planner stops carrying wide order rows under a Subquery Scan.
const FULL_PROJECTION: &str = "o.*,";

fn relation_sql(base_orders: &str) -> String {
    relation_sql_with_projection(base_orders, FULL_PROJECTION)
}

/// A narrow projection of the same shared relation: the columns the aggregate,
/// distinct and count tails read. The predicate set is byte-identical to
/// `public_order_sql` (same file, same composition point); only the projected
/// columns differ, so the planner stops carrying wide order rows beneath the
/// aggregation. Measured on the large fixture: the map/count/distinct tails'
/// statements dropped the Subquery Scan cost that made their p95 exceed
/// Python's inline-predicate plans.
fn relation_sql_with_projection(base_orders: &str, projection: &str) -> String {
    expand_shared_markers(
        &include_str!("../../sql/public_orders.sql")
            .replace(
                "/* current_order_rule */",
                include_str!("../../sql/current_order_rule.sql"),
            )
            .replace("/* shared_projection */o.*,", projection)
            .replace("/* base_orders */", base_orders),
        "$13",
    )
}

/// The complete bound public-order SELECT.
pub fn public_order_sql() -> &'static str {
    static SQL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| relation_sql("orderbook_orders"));
    &SQL
}

/// Rust decision 27: the benchmark's bound statement. The requested keys are
/// materialised once, the base scan is restricted to keyed rows before the
/// shared eligibility predicate, and the tail performs the final key join. The
/// full four-column key set is applied to the base scan (`requested_slices`
/// resolves the product code to its catalog id through the same attested
/// policy), so each keyed order enters once. The eligibility definition and
/// every predicate are the shared ones.
///
/// `requested_keys` stays MATERIALIZED because the tail and the slice
/// resolution both read it. `requested_slices` and `keyed_orders` are each
/// referenced once and are NOT MATERIALIZED, so the planner inlines them:
/// the decision-27 fences forced a nested loop over the keyed rows that
/// re-scanned `products`, `delivery_points` and the policy CTEs once per
/// row (~265 ms for 10,000 rows on the corpus). Inlined, the planner sees
/// the real `orderbook_orders` row counts and hash-joins the small
/// dimension tables. The predicate set — and therefore the row multiset —
/// is unchanged; `benchmark_key_first_matches_key_join_last` proves it.
pub fn benchmark_candidate_sql(tail: &'static str) -> OrderQuery {
    OrderQuery(expand_private_digest(&format!(
        "WITH requested_keys AS MATERIALIZED ({}) , requested_slices AS NOT MATERIALIZED ({}) , \
         keyed_orders AS NOT MATERIALIZED ({}) , eligible_orders AS NOT MATERIALIZED ({}) {}",
        include_str!("../../sql/requested_keys.sql"),
        include_str!("../../sql/requested_slices.sql"),
        include_str!("../../sql/keyed_orders.sql"),
        relation_sql("keyed_orders"),
        tail
    )))
}

/// The decision-27 fenced benchmark statement, byte for byte. Test-only: the
/// unfenced rewrite's differential test executes this historical shape and
/// requires the identical row multiset, so a planner-friendly change cannot
/// alter one returned row.
#[cfg(feature = "contract-tests")]
pub fn benchmark_candidate_sql_fenced(tail: &'static str) -> OrderQuery {
    OrderQuery(expand_private_digest(&format!(
        "WITH requested_keys AS MATERIALIZED ({}) , requested_slices AS MATERIALIZED ({}) , \
         keyed_orders AS MATERIALIZED ({}) , eligible_orders AS NOT MATERIALIZED ({}) {}",
        include_str!("../../sql/requested_keys.sql"),
        include_str!("../../sql/requested_slices.sql"),
        include_str!("../../sql/keyed_orders.sql"),
        relation_sql("keyed_orders"),
        tail
    )))
}

/// The complete bound formal-trade-evidence SELECT.
pub fn trade_evidence_sql() -> &'static str {
    static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "{} {}",
            include_str!("../../sql/formal_trade_snapshots.sql"),
            include_str!("../../sql/trade_evidence.sql")
        )
    });
    &SQL
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
pub struct OrderQuery(pub(super) String);
pub struct TradeQuery(String);

/// Rust decision 24: the one shared definition of the private digest input.
/// Consumers expand it for the rows they keep, never for the whole relation.
pub fn private_digest_input_sql(row_alias: &str) -> String {
    include_str!("../../sql/private_digest_input.sql").replace("/*row_alias*/", row_alias)
}

/// Replace every `/* private_digest_input(alias) */` marker with the shared
/// definition. One composition point keeps the digest bytes identical for
/// every consumer.
fn expand_private_digest(sql: &str) -> String {
    const PREFIX: &str = "/* private_digest_input(";
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(start) = rest.find(PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start + PREFIX.len()..];
        let end = after
            .find(") */")
            .expect("closed private digest placeholder");
        out.push_str(&private_digest_input_sql(&after[..end]));
        rest = &after[end + 4..];
    }
    out.push_str(rest);
    out
}

pub fn order_query_sql(tail: &'static str) -> OrderQuery {
    OrderQuery(expand_private_digest(&format!(
        "WITH eligible_orders AS NOT MATERIALIZED ({}) {}",
        public_order_sql(),
        tail
    )))
}

/// The shared relation with a narrow projection, for tails that aggregate or
/// deduplicate and never read an order's wide columns. The predicate set is
/// the shared one; only the projected columns differ from `order_query_sql`.
pub fn order_query_sql_projected(tail: &'static str, projection: &'static str) -> OrderQuery {
    OrderQuery(format!(
        "WITH eligible_orders AS NOT MATERIALIZED ({}) {}",
        relation_sql_with_projection("orderbook_orders", projection),
        tail
    ))
}

/// Same composition boundary for immutable historical evidence.
pub fn trade_query_sql(tail: &'static str) -> TradeQuery {
    TradeQuery(format!(
        // NOT MATERIALIZED lets the planner inline the relation and push a
        // tail's aggregation into the parallel workers. Materialised, the
        // reference aggregate pulled all 87.5k rows through a Subquery Scan
        // into one serial sort (measured 671 ms against Python's 289 ms on the
        // large fixture, where the same aggregation runs inside the workers).
        // The predicate set is unchanged.
        "WITH eligible_trades AS NOT MATERIALIZED ({}) {}",
        trade_evidence_sql(),
        tail
    ))
}

pub fn bind_order_query<'q>(
    sql: &OrderQuery,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
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
        .bind(filters.surface.as_str())
        .bind(config.kyc_execution_gate_enabled))
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
    config: &Config,
    filters: &PublicOrderFilters<'_>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let sql = order_query_sql("SELECT id FROM eligible_orders ORDER BY id LIMIT $14 OFFSET $15");
    let rows = bind_order_query(&sql, policy, clock, config, filters)?
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

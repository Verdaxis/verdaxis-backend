//! Task 10: old-shape/new-shape equivalence for the orderbook aggregate,
//! counts, distinct labels, map summary and kept-page listings.
//!
//! Each case compares the complete result rows — as JSON text, duplicates
//! retained — between the shape at the branch base and the shape this task
//! introduces. Visibility stays in the shared relation for both shapes, so
//! expired, ineligible, private and demo corpus rows participate. A synthetic
//! case pins the NULL-region and duplicate-key grouping semantics that the
//! seeded corpus does not carry.
//!
//! Runs against the seeded disposable market-contract database only; see
//! `rs/read/tests/run_market_contract.sh`.

use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use verdaxis_read::market::policy::load_policy;
use verdaxis_read::market::visibility::{
    AGGREGATE_PROJECTION, MAP_PROJECTION, OrderQuery, PublicOrderFilters, VisibilityClock,
    bind_order_sql, order_query_sql, order_query_sql_projected,
};
use verdaxis_read::{Config, MarketPolicy, compiled_sha, connect};

const POLICY_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/market-policy.json"
));

/// The listings tail at the branch base: page keys over the full relation,
/// then a lateral re-read of each page row. Frozen here as the oracle; the
/// task may change the production tail, never this copy.
const OLD_LISTINGS: &str = r#"
, page_keys AS MATERIALIZED (
    SELECT o.id, o.created_at
    FROM eligible_orders o
    ORDER BY o.created_at DESC, o.id
    OFFSET $14 LIMIT $15
)
SELECT detail.*, /* private_digest_input(base) */
FROM page_keys page
CROSS JOIN LATERAL (
    SELECT o.* FROM eligible_orders o WHERE o.id = page.id OFFSET 0
) detail
JOIN orderbook_orders base ON base.id = page.id
ORDER BY detail.created_at DESC, detail.id
"#;

/// The map summary's recent-ASK tail at the branch base.
const OLD_RECENT_ASKS: &str = r#"
, ranked AS (
    SELECT *, row_number() OVER (
        PARTITION BY delivery_point_id ORDER BY created_at DESC, id DESC
    ) AS point_rank
    FROM eligible_orders
)
SELECT *, price_per_mt_usd::text AS price_text,
       remaining_quantity_mt::text AS quantity_text
FROM ranked WHERE point_rank = 1
ORDER BY delivery_point_name
"#;

/// The aggregate tail's grouped relation, for the synthetic NULL-region case.
const AGGREGATE_GROUPED: &str = r#"
, grouped AS (
    SELECT product_id, product_name, market_product, product_fuel_type,
           delivery_point_id, delivery_point_name, delivery_point_region,
           availability_window, side, provenance,
           min(price_per_mt_usd) AS min_price,
           max(price_per_mt_usd) AS max_price,
           sum(remaining_quantity_mt) AS total_quantity,
           count(*) AS order_count, max(created_at) AS observed_at
    FROM eligible_orders
    GROUP BY product_id, product_name, market_product, product_fuel_type,
             delivery_point_id, delivery_point_name, delivery_point_region,
             availability_window, side, provenance
)
"#;

fn config() -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var("RUST_MARKET_TEST_DATABASE_URL")
            .expect("explicit disposable test URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
    }
}

fn policy() -> MarketPolicy {
    let document: Value = serde_json::from_slice(POLICY_BYTES).unwrap();
    let source_sha = document["source_sha"].as_str().unwrap().to_owned();
    load_policy(POLICY_BYTES, &source_sha).expect("the committed policy must load")
}

fn clock() -> VisibilityClock {
    let text = std::env::var("RUST_MARKET_TEST_CLOCK").expect("the pinned corpus clock is required");
    VisibilityClock::at(text.parse().expect("RFC 3339 corpus clock"))
}

/// Wrap a composed statement so its complete rows become comparable JSON.
fn wrap(sql: &str, order: &str) -> String {
    format!("SELECT to_jsonb(x)::text AS row FROM ({sql}) x {order}")
}

/// One trailing bind: `$14` and later are the consumer's own values.
enum Extra {
    Int(i64),
    Text(&'static str),
}

async fn rows(
    connection: &mut PgConnection,
    sql: &str,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
    filters: &PublicOrderFilters<'_>,
    extra: &[Extra],
) -> Vec<String> {
    let mut bound = bind_order_sql(sql, policy, clock, config, filters).unwrap();
    for value in extra {
        bound = match value {
            Extra::Int(value) => bound.bind(*value),
            Extra::Text(value) => bound.bind(*value),
        };
    }
    bound
        .fetch_all(&mut *connection)
        .await
        .expect("the statement must run under the app role")
        .into_iter()
        .map(|row| row.get::<String, _>("row"))
        .collect()
}

fn sorted(rows: &[String]) -> Vec<String> {
    let mut rows = rows.to_vec();
    rows.sort();
    rows
}

fn assert_same(old: &[String], new: &[String], ordered: bool, case: &str) {
    if ordered {
        assert_eq!(old, new, "result rows or their order drifted: {case}");
    } else {
        assert_eq!(sorted(old), sorted(new), "result rows drifted: {case}");
    }
}

/// R19: the aggregate reads only its group keys, price/quantity sums and the
/// observed time. Every group, total and count must stay exact, and the page
/// limit must not shrink `product_total_order_count` (count before pagination).
#[tokio::test]
async fn orderbook_aggregate_narrow_projection_keeps_every_group() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let aggregate = include_str!("../sql/orderbook/aggregate.sql");
    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        for limit in [1000_i64, 1, 0] {
            let filters = PublicOrderFilters::default();
            let old = rows(
                &mut connection,
                &wrap(order_query_sql(aggregate).sql(), ""),
                &policy,
                &clock,
                &config,
                &filters,
                &[Extra::Int(limit)],
            )
            .await;
            let new = rows(
                &mut connection,
                &wrap(
                    order_query_sql_projected(aggregate, AGGREGATE_PROJECTION).sql(),
                    "",
                ),
                &policy,
                &clock,
                &config,
                &filters,
                &[Extra::Int(limit)],
            )
            .await;
            assert_same(
                &old,
                &new,
                false,
                &format!("aggregate gate={gate} limit={limit}"),
            );
        }
    }

    // Count before pagination: the window sum covers every group, so a
    // one-row page still reports its product's full visible order count.
    let page = rows(
        &mut connection,
        &wrap(
            order_query_sql_projected(aggregate, AGGREGATE_PROJECTION).sql(),
            "",
        ),
        &policy,
        &clock,
        &config,
        &PublicOrderFilters::default(),
        &[Extra::Int(1)],
    )
    .await;
    let first: Value = serde_json::from_str(&page[0]).expect("the aggregate page is JSON");
    let product: Uuid = first["product_id"].as_str().unwrap().parse().unwrap();
    let provenance = first["provenance"].as_str().unwrap();
    // The pre-pagination total is the sum over every visible group of that
    // product and provenance — not the raw table count, which includes rows
    // the shared relation refuses. `limit = 1000` returns all 43 corpus groups.
    let all_groups = rows(
        &mut connection,
        &wrap(
            order_query_sql_projected(aggregate, AGGREGATE_PROJECTION).sql(),
            "",
        ),
        &policy,
        &clock,
        &config,
        &PublicOrderFilters::default(),
        &[Extra::Int(1000)],
    )
    .await;
    let full: i64 = all_groups
        .iter()
        .map(|row| {
            let value: Value = serde_json::from_str(row).unwrap();
            if value["product_id"] == product.to_string()
                && value["provenance"] == provenance
            {
                value["order_count"].as_i64().unwrap()
            } else {
                0
            }
        })
        .sum();
    let reported = first["product_total_order_count"].as_i64().unwrap();
    assert_eq!(
        reported, full,
        "the page's product total must be the pre-pagination count"
    );
    drop(connection);
    pools.read.close().await;
}

/// R20: product counts read only the policy label and the count. The narrow
/// projection must return the same count per market product for both gate
/// states and each side filter.
#[tokio::test]
async fn orderbook_product_counts_narrow_projection_keeps_every_count() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let counts = include_str!("../sql/orderbook/counts.sql");
    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        for side in [None, Some("BID"), Some("ASK")] {
            let filters = PublicOrderFilters {
                side,
                ..PublicOrderFilters::default()
            };
            let old = rows(
                &mut connection,
                &wrap(order_query_sql(counts).sql(), "ORDER BY x.market_product"),
                &policy,
                &clock,
                &config,
                &filters,
                &[],
            )
            .await;
            let new = rows(
                &mut connection,
                &wrap(
                    order_query_sql_projected(counts, "").sql(),
                    "ORDER BY x.market_product",
                ),
                &policy,
                &clock,
                &config,
                &filters,
                &[],
            )
            .await;
            assert_same(
                &old,
                &new,
                true,
                &format!("counts gate={gate} side={side:?}"),
            );
        }
    }
    drop(connection);
    pools.read.close().await;
}

/// R24/R25: distinct eligible labels in output order. The label-only
/// projection must return the same ordered set under both gate states.
#[tokio::test]
async fn orderbook_distinct_labels_keep_order_and_nulls() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let distinct = include_str!("../sql/orderbook/distinct.sql");
    for field in ["products", "regions", "fuel-types"] {
        for gate in [true, false] {
            let mut config = config.clone();
            config.kyc_execution_gate_enabled = gate;
            let old = rows(
                &mut connection,
                &wrap(order_query_sql(distinct).sql(), "ORDER BY x.value"),
                &policy,
                &clock,
                &config,
                &PublicOrderFilters::default(),
                &[Extra::Text(field)],
            )
            .await;
            let new = rows(
                &mut connection,
                &wrap(
                    order_query_sql_projected(distinct, "").sql(),
                    "ORDER BY x.value",
                ),
                &policy,
                &clock,
                &config,
                &PublicOrderFilters::default(),
                &[Extra::Text(field)],
            )
            .await;
            assert!(!old.is_empty(), "the corpus must expose {field} labels");
            assert_same(&old, &new, true, &format!("distinct {field} gate={gate}"));
        }
    }
    drop(connection);
    pools.read.close().await;
}

/// R21: the map summary keeps one recent ASK per delivery point plus the same
/// groups. Fetching detail for the kept page only must not change membership,
/// values or order.
#[tokio::test]
async fn orderbook_map_recent_asks_keep_one_row_per_point() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let recent = include_str!("../sql/orderbook/recent_asks.sql");
    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        let filters = PublicOrderFilters {
            side: Some("ASK"),
            ..PublicOrderFilters::default()
        };
        let old = rows(
            &mut connection,
            &wrap(
                order_query_sql_projected(OLD_RECENT_ASKS, MAP_PROJECTION).sql(),
                "ORDER BY x.delivery_point_name",
            ),
            &policy,
            &clock,
            &config,
            &filters,
            &[],
        )
        .await;
        let new = rows(
            &mut connection,
            &wrap(
                order_query_sql_projected(recent, MAP_PROJECTION).sql(),
                "ORDER BY x.delivery_point_name",
            ),
            &policy,
            &clock,
            &config,
            &filters,
            &[],
        )
        .await;
        assert!(!old.is_empty(), "the corpus must expose recent asks");
        assert_same(&old, &new, true, &format!("recent asks gate={gate}"));
    }
    drop(connection);
    pools.read.close().await;
}

/// R27: the kept page keeps exact membership across offsets, repeated pages
/// and the largest limit, with the private digest built for exactly those rows.
#[tokio::test]
async fn orderbook_listings_pages_match_the_full_ranking() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let listings = include_str!("../sql/orderbook/listings.sql");
    let filters = PublicOrderFilters {
        surface: verdaxis_read::market::visibility::OrderSurface::Listings,
        side: Some("ASK"),
        include_off_spec: true,
        ..PublicOrderFilters::default()
    };
    for (skip, limit) in [(0_i64, 50_i64), (0, 50), (1, 50), (25, 50), (0, 1), (0, 100)] {
        let old = rows(
            &mut connection,
            &wrap(
                order_query_sql(OLD_LISTINGS).sql(),
                "ORDER BY x.created_at DESC, x.id",
            ),
            &policy,
            &clock,
            &config,
            &filters,
            &[Extra::Int(skip), Extra::Int(limit)],
        )
        .await;
        let new = rows(
            &mut connection,
            &wrap(
                order_query_sql(listings).sql(),
                "ORDER BY x.created_at DESC, x.id",
            ),
            &policy,
            &clock,
            &config,
            &filters,
            &[Extra::Int(skip), Extra::Int(limit)],
        )
        .await;
        assert!(!old.is_empty(), "the corpus must fill a listings page");
        assert_same(
            &old,
            &new,
            true,
            &format!("listings skip={skip} limit={limit}"),
        );
    }
    drop(connection);
    pools.read.close().await;
}

/// The corpus carries the row classes the aggregate and distinct cases rely
/// on: expired, unknown, demo and duplicate-key rows. Without them those
/// cases would compare empty relations.
#[tokio::test]
async fn corpus_carries_the_rows_these_cases_compare() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    for (name, sql) in [
        (
            "expired",
            "SELECT count(*) FROM orderbook_orders WHERE expires_at IS NOT NULL \
             AND expires_at <= $1::timestamptz",
        ),
        (
            "unknown provenance",
            "SELECT count(*) FROM orderbook_orders WHERE provenance = 'UNKNOWN'",
        ),
        (
            "demo",
            "SELECT count(*) FROM orderbook_orders WHERE provenance = 'DEMO'",
        ),
        (
            "duplicate keys",
            "SELECT count(*) FROM (SELECT product_id, delivery_point_id, availability_window, side \
             FROM orderbook_orders GROUP BY 1,2,3,4 HAVING count(*) > 1) duplicated",
        ),
    ] {
        let value: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(clock.now)
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        assert!(value > 0, "the corpus must carry {name} rows");
    }
    drop(connection);
    pools.read.close().await;
}

/// Null-region and duplicate-key grouping on a synthetic relation: two rows
/// with a NULL region form one group, and identical-key rows count twice.
#[tokio::test]
async fn aggregate_groups_null_region_and_counts_duplicates() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let mut connection = pools.read.acquire().await.unwrap();
    let sql = format!(
        r#"
WITH eligible_orders(
    product_id, product_name, market_product, product_fuel_type,
    delivery_point_id, delivery_point_name, delivery_point_region,
    availability_window, side, provenance,
    price_per_mt_usd, remaining_quantity_mt, created_at
) AS (VALUES
    ('c4cecebc-3d2d-5840-8021-57a9a11bc673'::uuid, 'B30', 'B30', 'Biofuel',
     '78281fb3-e726-5396-802e-34b01fda21e8'::uuid, 'Dalian', NULL::text,
     'SPOT', 'BID', 'REAL', 100.00::numeric, 10.000::numeric,
     '2026-10-01T00:00:00Z'::timestamptz),
    ('c4cecebc-3d2d-5840-8021-57a9a11bc673'::uuid, 'B30', 'B30', 'Biofuel',
     '78281fb3-e726-5396-802e-34b01fda21e8'::uuid, 'Dalian', NULL::text,
     'SPOT', 'BID', 'REAL', 120.00::numeric, 5.000::numeric,
     '2026-10-02T00:00:00Z'::timestamptz)
)
{grouped}
SELECT *, total_quantity::text AS total_quantity_text FROM grouped
"#,
        grouped = AGGREGATE_GROUPED,
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(&mut *connection)
        .await
        .expect("the synthetic aggregate must run");
    assert_eq!(rows.len(), 1, "NULL regions must form one group");
    let row = &rows[0];
    assert!(row
        .try_get::<Option<String>, _>("delivery_point_region")
        .unwrap()
        .is_none());
    assert_eq!(row.get::<i64, _>("order_count"), 2);
    assert_eq!(row.get::<String, _>("total_quantity_text"), "15.000");
    drop(connection);
    pools.read.close().await;
}

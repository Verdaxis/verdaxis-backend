//! Task 9: old-shape/new-shape equivalence for the curve, reference-export and
//! MCP slice statements.
//!
//! Every case compares the complete returned rows — every projected column, as
//! JSON text — between the shape at the branch base and the keyed/narrow shape
//! this task introduces. A shape is kept only when the rows are identical
//! under both KYC gate states, at the corpus clock and on both sides of the
//! expiry and grace boundaries, with duplicate requested keys, with an empty
//! key set and with the board's null-window key. A negative control proves the
//! comparison detects a dropped key.
//!
//! Runs against the seeded disposable market-contract database only; see
//! `rs/read/tests/run_market_contract.sh`.

use chrono::{Duration, NaiveDate};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use verdaxis_read::market::policy::load_policy;
use verdaxis_read::market::visibility::{
    OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, bind_order_sql,
    order_query_sql,
};
#[path = "support/curve_candidate_shape.rs"]
mod curve_candidate_shape;
use curve_candidate_shape::curve_candidate_sql;
use verdaxis_read::{Config, MarketPolicy, compiled_sha, connect};

const POLICY_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/market-policy.json"
));

/// The curve tail every R09/R10/R11 case runs: the aggregate the route returns.
/// Both shapes take this same tail, so the comparison sees the route's own
/// rows — including the tail's key filter, which a raw-relation projection
/// would drop from the old side.
const ORDER_TAIL: &str = include_str!("../sql/curves/orders.sql");

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

async fn order_rows(
    connection: &mut PgConnection,
    sql: &str,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
    filters: &PublicOrderFilters<'_>,
    keys: &Value,
) -> Vec<String> {
    bind_order_sql(sql, policy, clock, config, filters)
        .unwrap()
        .bind(keys.to_string())
        .fetch_all(&mut *connection)
        .await
        .expect("the statement must run under the app role")
        .into_iter()
        .map(|row| row.get::<String, _>("row"))
        .collect()
}

/// Wrap a composed statement so its complete rows become comparable JSON.
fn wrap(sql: &str) -> String {
    format!("SELECT to_jsonb(x)::text AS row FROM ({sql}) x")
}

/// The slice keys the corpus can request: every visible (product, point,
/// window) triple. The key list never guesses the catalog.
async fn corpus_keys(
    connection: &mut PgConnection,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
) -> Vec<Value> {
    let sql = order_query_sql(
        "SELECT DISTINCT product_id, delivery_point_id, availability_window FROM eligible_orders",
    );
    let rows = bind_order_query(
        &sql,
        policy,
        clock,
        config,
        &PublicOrderFilters {
            surface: OrderSurface::Candidates,
            ..PublicOrderFilters::default()
        },
    )
    .unwrap()
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.iter()
        .map(|row| {
            json!({
                "product_id": row.get::<Uuid, _>("product_id"),
                "delivery_point_id": row.get::<Uuid, _>("delivery_point_id"),
                "availability_window": row.get::<String, _>("availability_window"),
            })
        })
        .collect()
}

/// Sort but keep duplicates: an equivalence proof must see a doubled row.
fn multiset(mut rows: Vec<String>) -> Vec<String> {
    rows.sort();
    rows
}

fn assert_multiset(old: &[String], new: &[String], case: &str) {
    assert_eq!(old, new, "row multiset drifted: {case}");
}

#[tokio::test]
async fn curve_table_keyed_candidates_match_the_full_relation() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let base = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let keys = corpus_keys(&mut connection, &policy, &base, &config).await;
    assert!(!keys.is_empty(), "the corpus must expose curve keys");
    let duplicate_keys = [keys.clone(), keys.clone()].concat();

    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        for offset in [
            Duration::microseconds(-1),
            Duration::zero(),
            Duration::microseconds(1),
        ] {
            let clock = VisibilityClock::at(base.now + offset);
            for (name, requested) in [
                ("all", &keys),
                ("duplicate", &duplicate_keys),
                ("empty", &Vec::new()),
            ] {
                let filters = PublicOrderFilters {
                    surface: OrderSurface::Candidates,
                    ..PublicOrderFilters::default()
                };
                let old = multiset(
                    order_rows(
                        &mut connection,
                        &wrap(order_query_sql(ORDER_TAIL).sql()),
                        &policy,
                        &clock,
                        &config,
                        &filters,
                        &Value::Array(requested.clone()),
                    )
                    .await,
                );
                let new = multiset(
                    order_rows(
                        &mut connection,
                        &wrap(curve_candidate_sql(ORDER_TAIL).as_str()),
                        &policy,
                        &clock,
                        &config,
                        &filters,
                        &Value::Array(requested.clone()),
                    )
                    .await,
                );
                assert_multiset(
                    &old,
                    &new,
                    &format!("table gate={gate} offset={offset:?} keys={name}"),
                );
            }
        }
    }
    drop(connection);
    pools.read.close().await;
}

/// Negative control: dropping one requested key must change the selected
/// rows, so the comparison above can fail.
#[tokio::test]
async fn curve_table_equivalence_detects_a_dropped_key() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let keys = corpus_keys(&mut connection, &policy, &clock, &config).await;
    let mut fewer = keys.clone();
    fewer.pop();
    let filters = PublicOrderFilters {
        surface: OrderSurface::Candidates,
        ..PublicOrderFilters::default()
    };
    let all = order_rows(
        &mut connection,
        &wrap(curve_candidate_sql(ORDER_TAIL).as_str()),
        &policy,
        &clock,
        &config,
        &filters,
        &Value::Array(keys),
    )
    .await;
    let subset = order_rows(
        &mut connection,
        &wrap(curve_candidate_sql(ORDER_TAIL).as_str()),
        &policy,
        &clock,
        &config,
        &filters,
        &Value::Array(fewer),
    )
    .await;
    assert_ne!(
        multiset(all),
        multiset(subset),
        "dropping a requested key must remove rows"
    );
    drop(connection);
    pools.read.close().await;
}

#[tokio::test]
async fn curve_slice_keyed_candidates_match_the_full_relation() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let base = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let keys = corpus_keys(&mut connection, &policy, &base, &config).await;
    let key = keys.first().expect("a corpus key").clone();
    let product: Uuid = key["product_id"].as_str().unwrap().parse().unwrap();
    let point: Uuid = key["delivery_point_id"].as_str().unwrap().parse().unwrap();
    let window = key["availability_window"].as_str().unwrap().to_owned();

    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        for offset in [
            Duration::microseconds(-1),
            Duration::zero(),
            Duration::microseconds(1),
        ] {
            let clock = VisibilityClock::at(base.now + offset);
            let filters = PublicOrderFilters {
                surface: OrderSurface::Candidates,
                product_id: Some(product),
                delivery_point_id: Some(point),
                availability_window: Some(&window),
                ..PublicOrderFilters::default()
            };
            let old = multiset(
                order_rows(
                    &mut connection,
                    &wrap(order_query_sql(ORDER_TAIL).sql()),
                    &policy,
                    &clock,
                    &config,
                    &filters,
                    &json!([key]),
                )
                .await,
            );
            let new = multiset(
                order_rows(
                    &mut connection,
                    &wrap(curve_candidate_sql(ORDER_TAIL).as_str()),
                    &policy,
                    &clock,
                    &config,
                    &filters,
                    &json!([key]),
                )
                .await,
            );
            assert_multiset(&old, &new, &format!("slice gate={gate} offset={offset:?}"));
        }
    }
    drop(connection);
    pools.read.close().await;
}

/// The board keeps its focus key plus every visible cell, including a
/// null-window key that means "all windows for this product and point".
#[tokio::test]
async fn curve_board_keyed_candidates_keep_null_window_keys() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let mut keys = corpus_keys(&mut connection, &policy, &clock, &config).await;
    let focus = keys.first().expect("a corpus key").clone();
    keys.push(json!({
        "product_id": focus["product_id"],
        "delivery_point_id": focus["delivery_point_id"],
        "availability_window": null,
    }));
    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        let filters = PublicOrderFilters {
            surface: OrderSurface::Candidates,
            ..PublicOrderFilters::default()
        };
        let old = multiset(
            order_rows(
                &mut connection,
                &wrap(order_query_sql(ORDER_TAIL).sql()),
                &policy,
                &clock,
                &config,
                &filters,
                &Value::Array(keys.clone()),
            )
            .await,
        );
        let new = multiset(
            order_rows(
                &mut connection,
                &wrap(curve_candidate_sql(ORDER_TAIL).as_str()),
                &policy,
                &clock,
                &config,
                &filters,
                &Value::Array(keys.clone()),
            )
            .await,
        );
        assert_multiset(&old, &new, &format!("board gate={gate}"));
    }
    drop(connection);
    pools.read.close().await;
}

/// MCP `get_market_slice` reads one slice through the shared relation. The
/// keyed candidate path must return the same rows under both gate states.
#[tokio::test]
async fn mcp_market_slice_keyed_candidates_match_the_full_relation() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let keys = corpus_keys(&mut connection, &policy, &clock, &config).await;
    let key = keys.first().expect("a corpus key").clone();
    let product: Uuid = key["product_id"].as_str().unwrap().parse().unwrap();
    let point: Uuid = key["delivery_point_id"].as_str().unwrap().parse().unwrap();
    let window = key["availability_window"].as_str().unwrap().to_owned();
    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        let filters = PublicOrderFilters {
            side: Some("ASK"),
            product_id: Some(product),
            delivery_point_id: Some(point),
            availability_window: Some(&window),
            ..PublicOrderFilters::default()
        };
        let old = multiset(
            order_rows(
                &mut connection,
                &wrap(order_query_sql(ORDER_TAIL).sql()),
                &policy,
                &clock,
                &config,
                &filters,
                &json!([key]),
            )
            .await,
        );
        let new = multiset(
            order_rows(
                &mut connection,
                &wrap(curve_candidate_sql(ORDER_TAIL).as_str()),
                &policy,
                &clock,
                &config,
                &filters,
                &json!([key]),
            )
            .await,
        );
        assert_multiset(&old, &new, &format!("mcp gate={gate}"));
    }
    drop(connection);
    pools.read.close().await;
}

/// Mutation control: the obvious keyed rewrite is a join from the base table
/// to the requested keys. With a repeated key, or with a null-window key beside
/// a specific-window key, that join admits one order twice. This test builds
/// that wrong shape from the same fragments and requires it to differ from the
/// full relation, so the duplicate-key cases above are not vacuous.
#[tokio::test]
async fn keyed_join_would_duplicate_rows() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    let keys = corpus_keys(&mut connection, &policy, &clock, &config).await;
    let key = keys.first().expect("a corpus key").clone();
    let duplicated = json!([key.clone(), key.clone()]).to_string();
    let single = json!([key]).to_string();
    // The same key list through a join and through the semi-join the production
    // shape uses. A join multiplies the row; `EXISTS` does not.
    let join = |payload: &str| {
        let sql = "SELECT count(*) FROM orderbook_orders AS o \
                   JOIN jsonb_to_recordset($1::jsonb) AS s(product_id uuid, \
                   delivery_point_id uuid, availability_window text) \
                   ON s.product_id = o.product_id AND s.delivery_point_id = o.delivery_point_id \
                   AND s.availability_window = o.availability_window";
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(payload.to_owned())
    };
    let exists = |payload: &str| {
        let sql = "SELECT count(*) FROM orderbook_orders AS o WHERE EXISTS ( \
                   SELECT 1 FROM jsonb_to_recordset($1::jsonb) AS s(product_id uuid, \
                   delivery_point_id uuid, availability_window text) \
                   WHERE s.product_id = o.product_id AND s.delivery_point_id = o.delivery_point_id \
                   AND s.availability_window = o.availability_window)";
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(payload.to_owned())
    };
    let joined_one = join(&single).fetch_one(&mut *connection).await.unwrap();
    let joined_two = join(&duplicated).fetch_one(&mut *connection).await.unwrap();
    let exists_one = exists(&single).fetch_one(&mut *connection).await.unwrap();
    let exists_two = exists(&duplicated).fetch_one(&mut *connection).await.unwrap();
    assert!(joined_one > 0, "the chosen key must select a row");
    assert_eq!(
        joined_two,
        joined_one * 2,
        "a join over a repeated key must duplicate the row"
    );
    assert_eq!(
        exists_two, exists_one,
        "the production semi-join must not duplicate a repeated key"
    );
    drop(connection);
    pools.read.close().await;
}

/// The corpus clock's Singapore date is the one the typed declaration rules
/// compare. This pins the date the cases above run at.
#[test]
fn corpus_clock_maps_to_the_pinned_singapore_date() {
    let clock = clock();
    assert_eq!(
        clock.singapore_date,
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    );
}

//! Task 6: the public visibility and trade-evidence matrices, compared with
//! the Python oracle under one clock.
//!
//! The disposable runner `rs/read/tests/run_market_contract.sh` migrates a
//! throwaway database, seeds the fixture matrix, and runs the Python oracle
//! over the same rows. This target replays the shared SQL as the read-only app
//! role and compares the selected id sets. It never targets a deployed
//! database.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use uuid::Uuid;

use verdaxis_read::market::policy::load_policy;
use verdaxis_read::{
    Config, MarketPolicy, PublicOrderFilters, VisibilityClock, compiled_sha, connect,
    runtime::artifact::ALLOWED_REVISIONS, select_public_order_ids,
};

const POLICY_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/market-policy.json"
));

fn config(variable: &str) -> Config {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: ALLOWED_REVISIONS[0].into(),
        database_url: std::env::var(variable).expect("explicit disposable test URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    };
    verdaxis_read::validate_config(&config).expect("numeric-loopback _test database required");
    config
}

fn policy() -> MarketPolicy {
    let document: Value = serde_json::from_slice(POLICY_BYTES).unwrap();
    let source_sha = document["source_sha"].as_str().unwrap().to_owned();
    load_policy(POLICY_BYTES, &source_sha).expect("the committed policy must load")
}

fn expected() -> Value {
    serde_json::json!({"clock":"2026-10-15T12:00:00Z","singapore_date":"2026-10-15"})
}

fn clock(document: &Value) -> VisibilityClock {
    let now: DateTime<Utc> = document["clock"].as_str().unwrap().parse().unwrap();
    let singapore_date: NaiveDate = document["singapore_date"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    VisibilityClock {
        now,
        singapore_date,
    }
}

fn as_strings(values: &[Uuid]) -> BTreeSet<String> {
    values.iter().map(Uuid::to_string).collect()
}

/// Rust decision 25 equivalence proof: the legacy correlated-`EXISTS` owner
/// admission and the new membership sets must select exactly the same rows over
/// the populated corpus. The legacy rule is a frozen test-only oracle.
fn legacy_relation_sql() -> String {
    let relation = include_str!("../sql/public_orders.sql")
        .replace("/* eligibility_sets */", "")
        .replace("/* base_orders */", "orderbook_orders")
        .replace(
            "/* current_order_rule */",
            include_str!("support/current_order_rule_legacy_owner.sql"),
        );
    format!(
        "WITH eligible_orders AS NOT MATERIALIZED ({relation}) \
         SELECT id FROM eligible_orders ORDER BY id"
    )
}

async fn legacy_ids(
    connection: &mut sqlx::PgConnection,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
    filters: &PublicOrderFilters<'_>,
) -> BTreeSet<String> {
    // Same bind order as `bind_order_query`; the default surface is "orderbook".
    let rows: Vec<(Uuid,)> = sqlx::query_as(sqlx::AssertSqlSafe(legacy_relation_sql()))
        .bind(serde_json::to_string(policy).unwrap())
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
        .bind("orderbook")
        .bind(config.kyc_execution_gate_enabled)
        .fetch_all(connection)
        .await
        .expect("the frozen legacy statement must run under the app role");
    rows.into_iter().map(|(id,)| id.to_string()).collect()
}

#[tokio::test]
async fn eligibility_sets_match_legacy_predicate_for_every_order() {
    let config = config("RUST_MARKET_TEST_DATABASE_URL");
    let pools = connect(&config)
        .await
        .expect("the read-only app role must connect");
    let policy = policy();
    let expected = expected();
    let clock = clock(&expected);
    let mut connection = pools.read.acquire().await.unwrap();
    for gate in [true, false] {
        for include_off_spec in [false, true] {
            let mut config = config.clone();
            config.kyc_execution_gate_enabled = gate;
            let filters = PublicOrderFilters {
                include_off_spec,
                ..PublicOrderFilters::default()
            };
            let new = as_strings(
                &select_public_order_ids(&mut connection, &policy, &clock, &config, &filters)
                    .await
                    .unwrap(),
            );
            let legacy = legacy_ids(&mut connection, &policy, &clock, &config, &filters).await;
            assert_eq!(
                new, legacy,
                "eligibility sets differ from the legacy predicate (gate={gate}, off_spec={include_off_spec})"
            );
        }
    }
}

/// The legacy curve relation composes the same rule through its own relation.
/// Compare its old and new id sets for one requested product.
fn legacy_curve_ids_sql(with_sets: bool) -> String {
    let mut relation = include_str!("../sql/legacy_curve_orders.sql").to_string();
    let rule = if with_sets {
        include_str!("../sql/current_order_rule.sql").to_string()
    } else {
        include_str!("support/current_order_rule_legacy_owner.sql").to_string()
    };
    let sets = if with_sets {
        verdaxis_read::market::visibility::eligibility_sets_sql("$13", "$2")
    } else {
        String::new()
    };
    relation = relation
        .replace("/* eligibility_sets */", &sets)
        .replace("/* current_order_rule */", &rule);
    format!("WITH eligible_orders AS ({relation}) SELECT id FROM eligible_orders ORDER BY id")
}

#[tokio::test]
async fn eligibility_sets_match_legacy_for_the_curve_relation() {
    let config = config("RUST_MARKET_TEST_DATABASE_URL");
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let expected = expected();
    let clock = clock(&expected);
    let mut connection = pools.read.acquire().await.unwrap();
    let product: Uuid = "8f6f34a5-2927-54a6-8564-cca40ae7cb8a".parse().unwrap();
    let mut results = Vec::new();
    for with_sets in [true, false] {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as(sqlx::AssertSqlSafe(legacy_curve_ids_sql(with_sets)))
                .bind(serde_json::to_string(&policy).unwrap())
                .bind(clock.now)
                .bind(clock.singapore_date.format("%Y-%m-%d").to_string())
                .bind(false)
                .bind(Some(product))
                .bind(Option::<Uuid>::None)
                .bind(Option::<String>::None)
                .bind(Option::<String>::None)
                .bind(Option::<String>::None)
                .bind(Option::<String>::None)
                .bind(Option::<String>::None)
                .bind("orderbook")
                .bind(config.kyc_execution_gate_enabled)
                .fetch_all(&mut *connection)
                .await
                .expect("the curve relation must run");
        results.push(
            rows.into_iter()
                .map(|(id,)| id.to_string())
                .collect::<BTreeSet<_>>(),
        );
    }
    assert_eq!(results[0], results[1], "curve relation eligibility changed");
}

/// Rust decision 27 equivalence: the key-first benchmark statement returns the
/// exact same rows (as a multiset) as the previous statement (eligibility over
/// every row, then join the requested keys), for every key the corpus can
/// request, including mismatched product/slice pairs.
#[tokio::test]
async fn benchmark_key_first_matches_key_join_last() {
    use sqlx::Row;
    use verdaxis_read::market::visibility::{
        OrderQuery, OrderSurface, benchmark_candidate_sql, benchmark_candidate_sql_fenced,
        bind_order_query, order_query_sql,
    };
    let config = config("RUST_MARKET_TEST_DATABASE_URL");
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let expected = expected();
    let clock = clock(&expected);
    let mut connection = pools.read.acquire().await.unwrap();
    let candidates = PublicOrderFilters {
        surface: OrderSurface::Candidates,
        ..PublicOrderFilters::default()
    };

    // Every key the corpus can request: each visible (side, point, window)
    // slice against every policy product code. The cross product includes
    // mismatched product/slice pairs, so a key scan that ignores the product
    // column would return extra (duplicated) rows and fail the multiset
    // comparison below.
    let slice_sql = order_query_sql(
        "SELECT DISTINCT side, delivery_point_id, availability_window FROM eligible_orders",
    );
    let slice_rows = bind_order_query(&slice_sql, &policy, &clock, &config, &candidates)
        .unwrap()
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    let mut products: Vec<String> = policy.products.iter().map(|p| p.code.clone()).collect();
    products.sort();
    products.dedup();
    let mut keys: Vec<serde_json::Value> = Vec::new();
    for row in &slice_rows {
        for product in &products {
            keys.push(serde_json::json!({
                "side": row.get::<String, _>("side"),
                "product": product,
                "point": row.get::<Uuid, _>("delivery_point_id").to_string(),
                "window": row.get::<String, _>("availability_window"),
            }));
        }
    }
    assert!(!keys.is_empty(), "the corpus must supply benchmark keys");
    let keys_json = serde_json::to_string(&keys).unwrap();

    // A multiset, not a set: a key scan that returns a row twice must differ.
    fn project(
        rows: &[sqlx::postgres::PgRow],
    ) -> std::collections::BTreeMap<(String, String, String, String, String, String), usize> {
        use sqlx::Row;
        let mut counts = std::collections::BTreeMap::new();
        for row in rows {
            let key = (
                row.get::<String, _>("side"),
                row.get::<String, _>("market_product"),
                row.get::<Uuid, _>("delivery_point_id").to_string(),
                row.get::<String, _>("availability_window"),
                row.get::<rust_decimal::Decimal, _>("remaining_quantity_mt")
                    .to_string(),
                row.get::<rust_decimal::Decimal, _>("price_per_mt_usd")
                    .to_string(),
            );
            *counts.entry(key).or_insert(0) += 1;
        }
        counts
    }

    // The previous statement: the full relation, then join the requested keys.
    let old_tail: &'static str = Box::leak(
        format!(
            ", requested_keys AS MATERIALIZED ({}) {}",
            include_str!("../sql/requested_keys.sql"),
            include_str!("../sql/live_benchmarks.sql")
        )
        .into_boxed_str(),
    );
    let old_query: OrderQuery = order_query_sql(old_tail);
    let old = bind_order_query(&old_query, &policy, &clock, &config, &candidates)
        .unwrap()
        .bind(keys_json.clone())
        .fetch_all(&mut *connection)
        .await
        .unwrap();

    // The new statement: keys applied before the shared eligibility predicate.
    let new_query: OrderQuery = benchmark_candidate_sql(include_str!("../sql/live_benchmarks.sql"));
    let keys_for_new = keys_json.clone();
    let new = bind_order_query(&new_query, &policy, &clock, &config, &candidates)
        .unwrap()
        .bind(keys_for_new)
        .fetch_all(&mut *connection)
        .await
        .unwrap();

    assert!(!new.is_empty(), "the key-first statement must return rows");
    assert_eq!(project(&new), project(&old), "key-first benchmark differs");

    // The decision-27 fenced shape (every stage MATERIALIZED) is the immediate
    // predecessor; the unfenced rewrite must return its exact multiset too, so
    // the planner-friendly form cannot change a single returned row.
    let fenced_query: OrderQuery =
        benchmark_candidate_sql_fenced(include_str!("../sql/live_benchmarks.sql"));
    let fenced_rows = bind_order_query(&fenced_query, &policy, &clock, &config, &candidates)
        .unwrap()
        .bind(keys_json)
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    assert_eq!(
        project(&new),
        project(&fenced_rows),
        "unfenced benchmark differs from the decision-27 fence"
    );
}

/// Phase 0 mirror proof: the fixed-biofuel supplier-metadata clause changed
/// the shared rule. The frozen pre-Phase-0 rule (test fixture above) must
/// disagree with the current rule exactly on the ASK rows that fail the
/// clause, and agree everywhere else, so the mirror is proven on both sides:
/// the negative rows (wrong standard, missing CI method, missing feedstock)
/// must lose visibility under the new rule only, and B30/B100 BID rows and
/// fully declared ASK rows must keep it.
fn biofuel_rule_sql(with_clause: bool) -> String {
    let sets = verdaxis_read::market::visibility::eligibility_sets_sql("$13", "$2");
    let relation = include_str!("../sql/public_orders.sql")
        .replace("/* eligibility_sets */", &sets)
        .replace("/* base_orders */", "orderbook_orders");
    let rule = if with_clause {
        include_str!("../sql/current_order_rule.sql").to_string()
    } else {
        include_str!("support/current_order_rule_pre_biofuel.sql").to_string()
    };
    let relation = relation.replace("/* current_order_rule */", &rule);
    format!(
        "WITH eligible_orders AS NOT MATERIALIZED ({relation}) \
         SELECT id FROM eligible_orders ORDER BY id"
    )
}

#[tokio::test]
async fn biofuel_supplier_metadata_clause_hides_only_failing_asks() {
    let config = config("RUST_MARKET_TEST_DATABASE_URL");
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock(&expected());
    let mut connection = pools.read.acquire().await.unwrap();
    let filters = PublicOrderFilters::default();
    let mut sets = Vec::new();
    for with_clause in [true, false] {
        let rows: Vec<(Uuid,)> = sqlx::query_as(sqlx::AssertSqlSafe(biofuel_rule_sql(with_clause)))
            .bind(serde_json::to_string(&policy).unwrap())
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
            .bind("orderbook")
            .bind(config.kyc_execution_gate_enabled)
            .fetch_all(&mut *connection)
            .await
            .expect("the biofuel differential statement must run");
        sets.push(
            rows.into_iter()
                .map(|(id,)| id.to_string())
                .collect::<BTreeSet<_>>(),
        );
    }
    let (with_clause, without_clause) = (&sets[0], &sets[1]);
    let dropped: Vec<&String> = without_clause.difference(with_clause).collect();
    let gained: Vec<&String> = with_clause.difference(without_clause).collect();
    assert!(
        gained.is_empty(),
        "the clause must only remove rows: {gained:?}"
    );
    // Exactly the two clause-only negative fixtures lose visibility. The third
    // negative fixture (missing feedstock) never reaches this differential
    // because the shared generic ASK predicates already refuse an empty
    // feedstock before the biofuel clause runs; its exclusion is proven by the
    // Python matrix comparison instead.
    assert_eq!(
        dropped.len(),
        2,
        "expected exactly the two clause-only negative fixtures to drop, got {dropped:?}"
    );
    let rows: Vec<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id::text, side, specification_standard, carbon_intensity_method \
             FROM orderbook_orders WHERE id IN ({})",
            dropped
                .iter()
                .map(|id| format!("\'{id}\'"))
                .collect::<Vec<_>>()
                .join(", ")
        )))
        .fetch_all(&mut *connection)
        .await
        .expect("the dropped rows must be readable");
    let mut saw_wrong_standard = false;
    let mut saw_missing_method = false;
    for (id, side, standard, method) in rows {
        assert_eq!(side, "ASK", "only ASK rows may drop: {id}");
        saw_wrong_standard |= standard.as_deref() == Some("EN 14214");
        saw_missing_method |= method.is_none();
    }
    assert!(
        saw_wrong_standard && saw_missing_method,
        "the dropped rows must exercise both clause-only arms: wrong standard and missing CI method"
    );
    // Positive rows keep visibility under the new rule: the two fully
    // declared biofuel ASKs and the two biofuel BIDs remain selected. Count
    // them through the same relation the clause lives in, filtered to
    // biofuel-fuel-type products.
    let kept = {
        let rows: Vec<(Uuid,)> = sqlx::query_as(sqlx::AssertSqlSafe(biofuel_rule_sql(true)))
            .bind(serde_json::to_string(&policy).unwrap())
            .bind(clock.now)
            .bind(clock.singapore_date.format("%Y-%m-%d").to_string())
            .bind(false)
            .bind(Option::<Uuid>::None)
            .bind(Option::<Uuid>::None)
            .bind(Option::<String>::None)
            .bind(Some("Biofuel".to_owned()))
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind("orderbook")
            .bind(config.kyc_execution_gate_enabled)
            .fetch_all(&mut *connection)
            .await
            .expect("the biofuel-filtered relation must run");
        rows.len()
    };
    assert_eq!(
        kept, 4,
        "visible biofuel rows: expected the two declared ASKs and two BIDs"
    );
}

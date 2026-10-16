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
    Config, MarketPolicy, PublicOrderFilters, TradeEvidenceFilters, VisibilityClock, compiled_sha,
    connect, select_public_order_ids, select_trade_evidence_ids,
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
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var(variable).expect("explicit disposable test URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        pool_max: 9,
        python_budget: 52,
        configured_max_connections: 100,
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
    let path =
        std::env::var("RUST_MARKET_TEST_EXPECTED").expect("the oracle output path is required");
    let text = std::fs::read(&path).expect("the oracle output must exist");
    serde_json::from_slice(&text).expect("the oracle output must be valid JSON")
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

fn ids(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .expect("an id list is required")
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn as_strings(values: &[Uuid]) -> BTreeSet<String> {
    values.iter().map(Uuid::to_string).collect()
}

#[tokio::test]
async fn public_visibility_matrix_matches_python() {
    let config = config("RUST_MARKET_TEST_DATABASE_URL");
    let pools = connect(&config)
        .await
        .expect("the read-only app role must connect");
    let policy = policy();
    let expected = expected();
    let clock = clock(&expected);
    for name in [
        "rejected_with_grace",
        "wrong_kyc_org",
        "wrong_grace_org",
        "grace_before",
        "grace_after",
        "org_revoked",
        "assisted_rejected",
        "assisted_mismatched",
        "admin_kyc_only",
        "expiry_equal",
        "demo_null_expiry",
        "zero_remaining",
        "typed_invalid_visible",
        "fame_wrong_lane",
        "lng_wrong_lane",
    ] {
        assert!(
            expected["cases"].get(name).is_some(),
            "missing acceptance case {name}"
        );
    }
    let mut connection = pools.read.acquire().await.unwrap();

    for (include_off_spec, key) in [(false, "selected"), (true, "selected_with_off_spec")] {
        let filters = PublicOrderFilters {
            include_off_spec,
            ..PublicOrderFilters::default()
        };
        let actual = as_strings(
            &select_public_order_ids(&mut connection, &policy, &clock, &config, &filters)
                .await
                .expect("the bound statement must run under the app role"),
        );
        assert_eq!(actual, ids(&expected[key]), "visibility mismatch for {key}");
    }

    for run in expected["kyc_gate_runs"].as_array().unwrap() {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = run["enabled"].as_bool().unwrap();
        let actual = select_public_order_ids(
            &mut connection,
            &policy,
            &clock,
            &config,
            &PublicOrderFilters::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            as_strings(&actual),
            ids(&run["selected"]),
            "KYC gate {}",
            run["enabled"]
        );
    }

    for case in expected["region_filters"].as_array().unwrap() {
        let filters = PublicOrderFilters {
            region: case["region"].as_str(),
            ..PublicOrderFilters::default()
        };
        let actual = select_public_order_ids(&mut connection, &policy, &clock, &config, &filters)
            .await
            .unwrap();
        assert_eq!(
            as_strings(&actual),
            ids(&case["ids"]),
            "region {}",
            case["region"]
        );
    }

    for case in expected["lane_filters"].as_array().unwrap() {
        let filters = PublicOrderFilters {
            product_id: Some(case["product"].as_str().unwrap().parse().unwrap()),
            delivery_point_id: Some(case["point"].as_str().unwrap().parse().unwrap()),
            ..PublicOrderFilters::default()
        };
        let actual = select_public_order_ids(&mut connection, &policy, &clock, &config, &filters)
            .await
            .unwrap();
        assert_eq!(as_strings(&actual), ids(&case["ids"]));
    }
    for name in [
        "canary_org",
        "unregistered_test_org",
        "mixed_trade",
        "legacy_trade_insert",
    ] {
        assert_eq!(expected["cases"][name]["guard_refused"], true);
    }

    // M4: the compatibility surface is intentionally less restrictive.
    let listing_filters = PublicOrderFilters {
        surface: verdaxis_read::market::visibility::OrderSurface::Listings,
        side: Some("ASK"),
        include_off_spec: true,
        ..PublicOrderFilters::default()
    };
    let listings =
        select_public_order_ids(&mut connection, &policy, &clock, &config, &listing_filters)
            .await
            .unwrap();
    assert_eq!(
        as_strings(&listings),
        ids(&expected["listings"]),
        "listings qualification differs"
    );

    use sqlx::Row;
    use verdaxis_read::market::visibility::{bind_order_query, order_query_sql};
    let depth_sql = order_query_sql(
        ", ranked AS (SELECT id, row_number() OVER (PARTITION BY side ORDER BY price_per_mt_usd, id) AS rank FROM eligible_orders) SELECT id FROM ranked WHERE rank <= 2",
    );
    let depth = bind_order_query(
        &depth_sql,
        &policy,
        &clock,
        &config,
        &PublicOrderFilters::default(),
    )
    .unwrap()
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let depth: Vec<Uuid> = depth.iter().map(|row| row.get("id")).collect();
    assert_eq!(as_strings(&depth), ids(&expected["depth"]));
    // Rust decision 24: the shared relation no longer carries the digest input.
    // Expand the one shared definition for the rows this consumer keeps.
    let sql = order_query_sql(
        "SELECT o.id, /* private_digest_input(o) */ FROM eligible_orders o ORDER BY o.id",
    );
    let rows = bind_order_query(
        &sql,
        &policy,
        &clock,
        &config,
        &PublicOrderFilters::default(),
    )
    .unwrap()
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    for row in rows {
        let input: Value = row.get("private_digest_input");
        let canonical = verdaxis_read::canonical_terms_from_row(&input).unwrap();
        let order = verdaxis_read::QualificationInput {
            product_id: input["product_id"].as_str().map(|id| id.parse().unwrap()),
            market_product: None,
            side: input["side"].as_str().unwrap(),
            delivery_point_id: input["delivery_point_id"].as_str(),
            availability_window: input["availability_window"].as_str().unwrap(),
            off_spec: input["off_spec"].as_bool().unwrap(),
            msds_available: input["msds_available"].as_bool().unwrap(),
            certification_scheme: input["certification_scheme"].as_str(),
            certification_declared: input["certification_declared"].as_bool().unwrap(),
            fame_terms: input.get("fame_terms").filter(|value| !value.is_null()),
            lng_terms: input.get("lng_terms").filter(|value| !value.is_null()),
        };
        assert_eq!(
            verdaxis_read::order_is_execution_qualified_at(&policy, &order, clock.singapore_date),
            expected["qualification"][input["id"].as_str().unwrap()]
        );

        assert_eq!(
            canonical,
            expected["digest_inputs"][input["id"].as_str().unwrap()]
        );
    }
    let sql = order_query_sql(
        "SELECT id FROM eligible_orders ORDER BY created_at DESC, id LIMIT $14 OFFSET $15",
    );
    let rows = bind_order_query(&sql, &policy, &clock, &config, &listing_filters)
        .unwrap()
        .bind(2_i64)
        .bind(1_i64)
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    let actual: Vec<String> = rows
        .iter()
        .map(|row| row.get::<Uuid, _>("id").to_string())
        .collect();
    let wanted: Vec<String> = expected["listings"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .take(2)
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, wanted, "ordered listing page");

    // The pool close waits for every connection to return, so the borrowed
    // connection must go out of scope first.
    drop(connection);
    pools.read.close().await;
}

#[tokio::test]
async fn trade_evidence_matrix_matches_python() {
    let config = config("RUST_MARKET_TEST_DATABASE_URL");
    let pools = connect(&config)
        .await
        .expect("the read-only app role must connect");
    let policy = policy();
    let expected = expected();
    let mut connection = pools.read.acquire().await.unwrap();

    let actual = as_strings(
        &select_trade_evidence_ids(&mut connection, &policy, &TradeEvidenceFilters::default())
            .await
            .expect("the bound statement must run under the app role"),
    );
    assert_eq!(
        actual,
        ids(&expected["trade_selected"]),
        "trade evidence mismatch"
    );

    for (key, since, before) in [
        ("since_equal", Some(clock(&expected).now), None),
        ("before_equal", None, Some(clock(&expected).now)),
    ] {
        let filters = TradeEvidenceFilters {
            confirmed_since: since,
            confirmed_before: before,
            ..TradeEvidenceFilters::default()
        };
        let actual = select_trade_evidence_ids(&mut connection, &policy, &filters)
            .await
            .unwrap();
        assert_eq!(as_strings(&actual), ids(&expected["cutoffs"][key]));
    }
    drop(connection);
    pools.read.close().await;
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
        include_str!("../sql/eligibility_sets.sql").replace("/*gate*/", "$13")
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
        OrderQuery, OrderSurface, benchmark_candidate_sql, bind_order_query, order_query_sql,
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
    let new = bind_order_query(&new_query, &policy, &clock, &config, &candidates)
        .unwrap()
        .bind(keys_json)
        .fetch_all(&mut *connection)
        .await
        .unwrap();

    assert!(!new.is_empty(), "the key-first statement must return rows");
    assert_eq!(project(&new), project(&old), "key-first benchmark differs");
}

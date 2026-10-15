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
            &select_public_order_ids(&mut connection, &policy, &clock, &filters)
                .await
                .expect("the bound statement must run under the app role"),
        );
        assert_eq!(actual, ids(&expected[key]), "visibility mismatch for {key}");
    }

    for case in expected["lane_filters"].as_array().unwrap() {
        let filters = PublicOrderFilters {
            product_id: Some(case["product"].as_str().unwrap().parse().unwrap()),
            delivery_point_id: Some(case["point"].as_str().unwrap().parse().unwrap()),
            ..PublicOrderFilters::default()
        };
        let actual = select_public_order_ids(&mut connection, &policy, &clock, &filters)
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
    let listings = select_public_order_ids(&mut connection, &policy, &clock, &listing_filters)
        .await
        .unwrap();
    assert_eq!(
        as_strings(&listings),
        ids(&expected["listings"]),
        "listings qualification differs"
    );

    use sqlx::Row;
    use verdaxis_read::market::visibility::{bind_order_query, order_query_sql};
    let sql = order_query_sql("SELECT private_digest_input FROM eligible_orders ORDER BY id");
    let rows = bind_order_query(&sql, &policy, &clock, &PublicOrderFilters::default())
        .unwrap()
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    for row in rows {
        let input: Value = row.get("private_digest_input");
        let canonical = verdaxis_read::canonical_terms_from_row(&input).unwrap();
        assert_eq!(
            canonical,
            expected["digest_inputs"][input["id"].as_str().unwrap()]
        );
    }
    let sql = order_query_sql(
        "SELECT id FROM eligible_orders ORDER BY created_at DESC, id LIMIT $13 OFFSET $14",
    );
    let rows = bind_order_query(&sql, &policy, &clock, &listing_filters)
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

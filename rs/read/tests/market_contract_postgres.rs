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

    drop(connection);
    pools.read.close().await;
}

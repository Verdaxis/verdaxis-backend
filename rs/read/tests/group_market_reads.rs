//! Cache, public market read and contract tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

// Several test files here load the same small helper from support/ as their own
// module, as they did as separate binaries, so the test files stay unchanged.
#![allow(clippy::duplicate_mod)]

#[path = "cache_authority.rs"]
pub mod cache_authority;
#[path = "cache_events.rs"]
pub mod cache_events;
#[path = "cache_events_postgres.rs"]
pub mod cache_events_postgres;
#[path = "cache_failover_postgres.rs"]
pub mod cache_failover_postgres;
#[path = "curves_canonical.rs"]
pub mod curves_canonical;
#[path = "curves_legacy.rs"]
pub mod curves_legacy;
#[path = "live_benchmarks.rs"]
pub mod live_benchmarks;
#[path = "market_contract.rs"]
pub mod market_contract;
#[cfg(feature = "contract-tests")]
#[path = "market_contract_postgres.rs"]
pub mod market_contract_postgres;
#[path = "mcp.rs"]
pub mod mcp;
#[path = "mcp_validation.rs"]
pub mod mcp_validation;
#[cfg(feature = "contract-tests")]
#[path = "meaning_curve_queries.rs"]
pub mod meaning_curve_queries;
#[path = "meaning_decimal.rs"]
pub mod meaning_decimal;
#[cfg(feature = "contract-tests")]
#[path = "meaning_orderbook_queries.rs"]
pub mod meaning_orderbook_queries;
#[path = "orderbook_lists.rs"]
pub mod orderbook_lists;
#[path = "orderbook_map.rs"]
pub mod orderbook_map;
#[path = "orderbook_snapshot.rs"]
pub mod orderbook_snapshot;
#[path = "price_alert_evaluation.rs"]
pub mod price_alert_evaluation;
// Runs in the PostgreSQL job: it reads RUST_TEST_MIGRATOR_DATABASE_URL.
#[cfg(feature = "contract-tests")]
#[path = "price_alert_evaluation_postgres.rs"]
pub mod price_alert_evaluation_postgres;
#[path = "price_decimal.rs"]
pub mod price_decimal;
#[path = "price_headers.rs"]
pub mod price_headers;
#[path = "prices.rs"]
pub mod prices;
#[path = "quota_contract.rs"]
pub mod quota_contract;
#[path = "reference_cache.rs"]
pub mod reference_cache;
#[path = "simple_reads.rs"]
pub mod simple_reads;
#[path = "trading_cache_postgres.rs"]
pub mod trading_cache_postgres;
#[path = "voyage.rs"]
pub mod voyage;

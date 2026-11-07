//! Phase 10 deploy, operations and unit tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[path = "phase10_auth_maintenance_postgres.rs"]
mod phase10_auth_maintenance_postgres;
#[path = "phase10_baseline_postgres.rs"]
mod phase10_baseline_postgres;
#[path = "phase10_cache.rs"]
mod phase10_cache;
#[path = "phase10_cache_postgres.rs"]
mod phase10_cache_postgres;
#[path = "phase10_canary.rs"]
mod phase10_canary;
#[path = "phase10_canary_postgres.rs"]
mod phase10_canary_postgres;
#[path = "phase10_cli.rs"]
mod phase10_cli;
#[path = "phase10_external.rs"]
mod phase10_external;
#[path = "phase10_external_checks.rs"]
mod phase10_external_checks;
// Phase 10 Task 22: the GENA producer-project import.
#[cfg(feature = "contract-tests")]
#[path = "phase10_gena.rs"]
mod phase10_gena;
#[cfg(feature = "contract-tests")]
#[path = "phase10_gena_postgres.rs"]
mod phase10_gena_postgres;
#[path = "phase10_guard.rs"]
mod phase10_guard;
#[path = "phase10_health.rs"]
mod phase10_health;
// Phase 10 Task 21: trusted signal ingestion (offline and PostgreSQL cases).
#[cfg(feature = "contract-tests")]
#[path = "phase10_ingest.rs"]
mod phase10_ingest;
#[cfg(feature = "contract-tests")]
#[path = "phase10_ingest_postgres.rs"]
mod phase10_ingest_postgres;
#[path = "phase10_inspect.rs"]
mod phase10_inspect;
#[path = "phase10_inspect_postgres.rs"]
mod phase10_inspect_postgres;
#[path = "phase10_kyc_postgres.rs"]
mod phase10_kyc_postgres;
#[path = "phase10_migrate_postgres.rs"]
mod phase10_migrate_postgres;
#[path = "phase10_news.rs"]
mod phase10_news;
#[cfg(feature = "contract-tests")]
#[path = "phase10_news_postgres.rs"]
mod phase10_news_postgres;
#[path = "phase10_onboarding.rs"]
mod phase10_onboarding;
#[path = "phase10_onboarding_postgres.rs"]
mod phase10_onboarding_postgres;
// Phase 10 Task 9: purpose-bound runtime environments; the parity cases run
// the Python projection from RUST_TEST_PYTHON.
#[cfg(feature = "contract-tests")]
#[path = "phase10_ops_env.rs"]
mod phase10_ops_env;
#[cfg(feature = "contract-tests")]
#[path = "phase10_ops_preflight_postgres.rs"]
mod phase10_ops_preflight_postgres;
#[path = "phase10_prune_postgres.rs"]
mod phase10_prune_postgres;
#[path = "phase10_reminders_postgres.rs"]
mod phase10_reminders_postgres;
#[path = "phase10_root.rs"]
mod phase10_root;
#[path = "phase10_state.rs"]
mod phase10_state;
#[cfg(feature = "contract-tests")]
#[path = "phase10_state_postgres.rs"]
mod phase10_state_postgres;
#[path = "phase10_units.rs"]
mod phase10_units;
#[path = "phase10_watchdog.rs"]
mod phase10_watchdog;

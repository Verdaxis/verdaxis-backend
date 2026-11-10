//! Offboarding tests, offline and PostgreSQL, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary.

// Several test files here load the same small helper from support/ as their own
// module, as they did as separate binaries, so the test files stay unchanged.
#![allow(clippy::duplicate_mod)]

#[path = "offboarding_acceptance.rs"]
pub mod offboarding_acceptance;
#[path = "offboarding_acceptance_postgres.rs"]
pub mod offboarding_acceptance_postgres;
#[path = "offboarding_admission.rs"]
pub mod offboarding_admission;
#[path = "offboarding_admission_postgres.rs"]
pub mod offboarding_admission_postgres;
#[path = "offboarding_command.rs"]
pub mod offboarding_command;
#[path = "offboarding_command_postgres.rs"]
pub mod offboarding_command_postgres;
#[path = "offboarding_composition.rs"]
pub mod offboarding_composition;
#[path = "offboarding_composition_postgres.rs"]
pub mod offboarding_composition_postgres;
#[path = "offboarding_credentials.rs"]
pub mod offboarding_credentials;
#[path = "offboarding_credentials_postgres.rs"]
pub mod offboarding_credentials_postgres;
#[path = "offboarding_market.rs"]
pub mod offboarding_market;
#[path = "offboarding_market_postgres.rs"]
pub mod offboarding_market_postgres;
#[path = "offboarding_reassignment.rs"]
pub mod offboarding_reassignment;
#[path = "offboarding_reassignment_postgres.rs"]
pub mod offboarding_reassignment_postgres;
#[path = "offboarding_schema.rs"]
pub mod offboarding_schema;
#[path = "offboarding_schema_postgres.rs"]
pub mod offboarding_schema_postgres;

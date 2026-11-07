//! Offboarding tests, offline and PostgreSQL, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary.

// Several test files here load the same small helper from support/ as their own
// module, as they did as separate binaries, so the test files stay unchanged.
#![allow(clippy::duplicate_mod)]

#[path = "offboarding_acceptance.rs"]
mod offboarding_acceptance;
#[path = "offboarding_acceptance_postgres.rs"]
mod offboarding_acceptance_postgres;
#[path = "offboarding_admission.rs"]
mod offboarding_admission;
#[path = "offboarding_admission_postgres.rs"]
mod offboarding_admission_postgres;
#[path = "offboarding_command.rs"]
mod offboarding_command;
#[path = "offboarding_command_postgres.rs"]
mod offboarding_command_postgres;
#[path = "offboarding_composition.rs"]
mod offboarding_composition;
#[path = "offboarding_composition_postgres.rs"]
mod offboarding_composition_postgres;
#[path = "offboarding_credentials.rs"]
mod offboarding_credentials;
#[path = "offboarding_credentials_postgres.rs"]
mod offboarding_credentials_postgres;
#[path = "offboarding_market.rs"]
mod offboarding_market;
#[path = "offboarding_market_postgres.rs"]
mod offboarding_market_postgres;
#[path = "offboarding_reassignment.rs"]
mod offboarding_reassignment;
#[path = "offboarding_reassignment_postgres.rs"]
mod offboarding_reassignment_postgres;
#[path = "offboarding_schema.rs"]
mod offboarding_schema;
#[path = "offboarding_schema_postgres.rs"]
mod offboarding_schema_postgres;

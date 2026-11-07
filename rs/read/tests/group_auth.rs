//! Authentication, session and token tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary.

#[path = "auth_coexistence_postgres.rs"]
mod auth_coexistence_postgres;
#[path = "auth_expiry.rs"]
mod auth_expiry;
#[path = "auth_http.rs"]
mod auth_http;
#[path = "auth_issue.rs"]
mod auth_issue;
#[path = "auth_login.rs"]
mod auth_login;
#[path = "auth_login_postgres.rs"]
mod auth_login_postgres;
#[path = "auth_password.rs"]
mod auth_password;
#[path = "auth_races_postgres.rs"]
mod auth_races_postgres;
#[path = "auth_revocation_postgres.rs"]
mod auth_revocation_postgres;
#[path = "auth_store_postgres.rs"]
mod auth_store_postgres;
#[path = "auth_token_types.rs"]
mod auth_token_types;
#[path = "auth_tokens.rs"]
mod auth_tokens;
#[path = "auth_transaction_panic.rs"]
mod auth_transaction_panic;
#[path = "auth_transactions.rs"]
mod auth_transactions;

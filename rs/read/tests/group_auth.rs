//! Authentication, session and token tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary.

#[path = "auth_coexistence_postgres.rs"]
pub mod auth_coexistence_postgres;
#[path = "auth_expiry.rs"]
pub mod auth_expiry;
#[path = "auth_http.rs"]
pub mod auth_http;
#[path = "auth_issue.rs"]
pub mod auth_issue;
#[path = "auth_login.rs"]
pub mod auth_login;
#[path = "auth_login_postgres.rs"]
pub mod auth_login_postgres;
#[path = "auth_password.rs"]
pub mod auth_password;
#[path = "auth_races_postgres.rs"]
pub mod auth_races_postgres;
#[path = "auth_revocation_postgres.rs"]
pub mod auth_revocation_postgres;
#[path = "auth_store_postgres.rs"]
pub mod auth_store_postgres;
#[path = "auth_token_types.rs"]
pub mod auth_token_types;
#[path = "auth_tokens.rs"]
pub mod auth_tokens;
#[path = "auth_transaction_panic.rs"]
pub mod auth_transaction_panic;
#[path = "auth_transactions.rs"]
pub mod auth_transactions;

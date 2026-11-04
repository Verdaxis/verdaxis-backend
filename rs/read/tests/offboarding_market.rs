//! Offline contract of the individual market scope. The behaviour runs in
//! `offboarding_market_postgres`.

use std::path::{Path, PathBuf};

use sqlx::PgConnection;
use verdaxis_read::order_write::{
    InvalidateUser, InvalidationError, InvalidationResult, invalidate_user,
};

fn boundary() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/order_write")
}

/// The files the borrowed invalidation runs through.
const SUBCOMMAND_FILES: [&str; 4] = [
    "invalidation.rs",
    "events.rs",
    "reservations.rs",
    "benchmarks.rs",
];

#[test]
fn user_scope_borrows_the_caller_transaction() {
    // The subcommand takes the caller's connection and returns its effects;
    // it cannot own a pool, a transaction or the commit.
    async fn call(
        conn: &mut PgConnection,
        command: &InvalidateUser,
    ) -> Result<InvalidationResult, InvalidationError> {
        invalidate_user(conn, command).await
    }
    let _ = call;
}

#[test]
fn invalidation_never_completes_a_transaction() {
    let words = |text: &str| -> Vec<String> {
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect()
    };
    for file in SUBCOMMAND_FILES {
        let text = std::fs::read_to_string(boundary().join(file)).unwrap();
        for call in [".commit(", ".rollback(", ".begin(", "begin_with("] {
            assert!(!text.contains(call), "{file} calls {call}");
        }
        for statement in ["COMMIT", "ROLLBACK", "SAVEPOINT"] {
            assert!(
                !words(&text).iter().any(|word| word == statement),
                "{file} issues {statement}"
            );
        }
    }
}

#[test]
fn both_collections_are_bounded_at_one_hundred() {
    let text = std::fs::read_to_string(boundary().join("invalidation.rs")).unwrap();
    assert!(text.contains("const MAX_INVALIDATION_ROWS: usize = 100;"));
    // The bound refuses: no statement of the scope is cut short by LIMIT 100.
    assert!(!text.contains("LIMIT 100"));
    assert!(text.contains("MAX_INVALIDATION_ROWS + 1"));
}

//! Phase 10 Task 23: read-only inspection tools against a migrated disposable
//! database. Run through `run_phase10_inspect.sh`, which builds the schema with
//! the migration tool, loads the fixtures in `tests/fixtures/phase10_inspect_*`
//! and starts the Python report as the reference.
//!
//! Three connections are used on purpose:
//! - the reader role holds `SELECT` only and runs the commands;
//! - the owner role can write, and proves the transaction (not a missing
//!   privilege) is what refuses a write;
//! - a second database holds a state with no blocker.
#![cfg(feature = "contract-tests")]

use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use std::collections::BTreeMap;
use verdaxis_read::cli::ops::inspect::{begin_read_only, collect_blockers, collect_users, execute};
use verdaxis_read::cli::{Environment, Exit, IdentityArgs, InspectArgs, InspectKind};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn url(variable: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must name a disposable database"))
}

async fn pool(variable: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url(variable))
        .await
        .expect("connect a disposable database")
}

fn arguments(kind: InspectKind, json: bool, show_emails: bool) -> InspectArgs {
    InspectArgs {
        kind,
        identity: IdentityArgs {
            environment: Environment::Staging,
            release_sha: SHA.to_owned(),
        },
        json,
        show_emails,
    }
}

struct Run {
    exit: Exit,
    stdout: String,
    stderr: String,
}

async fn run(pool: &PgPool, args: &InspectArgs) -> Run {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = execute(pool, args, &mut stdout, &mut stderr).await;
    Run {
        exit,
        stdout: String::from_utf8(stdout).expect("utf8 report"),
        stderr: String::from_utf8(stderr).expect("utf8 summary"),
    }
}

/// `name=value` lines other than the per-user lines.
fn values(report: &str) -> BTreeMap<String, String> {
    report
        .lines()
        .filter(|line| !line.starts_with("user="))
        .map(|line| {
            let (name, value) = line.split_once('=').expect("name=value line");
            (name.to_owned(), value.to_owned())
        })
        .collect()
}

fn user_lines(report: &str) -> Vec<String> {
    let mut lines: Vec<String> = report
        .lines()
        .filter(|line| line.starts_with("user="))
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

// --- Masking, end to end -----------------------------------------------

#[tokio::test]
async fn inspect_users_masks_email_by_default() {
    let pool = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let text = run(&pool, &arguments(InspectKind::Users, false, false)).await;
    assert_eq!(text.exit, Exit::Ok);
    assert!(
        text.stdout.starts_with("Found 12 users:\n"),
        "{}",
        text.stdout
    );
    assert!(
        text.stdout
            .contains(" - a***@example.test (Role: UserRole.BUYER)")
    );
    for local_part in [
        "alice.buyer",
        "bob.supplier",
        "carol",
        "frank.admin",
        "dup@",
    ] {
        assert!(!text.stdout.contains(local_part), "{local_part} leaked");
        assert!(!text.stderr.contains(local_part), "{local_part} leaked");
    }
    // Rows are listed in a fixed order, so the report repeats exactly.
    let again = run(&pool, &arguments(InspectKind::Users, false, false)).await;
    assert_eq!(text.stdout, again.stdout);

    let json = run(&pool, &arguments(InspectKind::Users, true, false)).await;
    let report: Value = serde_json::from_str(&json.stdout).expect("one JSON object");
    assert_eq!(report["emails"], "masked");
    for user in report["users"].as_array().expect("users") {
        let email = user["email"].as_str().expect("email");
        assert!(email.contains("***@"), "{email}");
        assert!(!email.contains("@example.test") || email.contains("***@example.test"));
    }
    assert!(!json.stdout.contains("alice.buyer"));
}

#[tokio::test]
async fn inspect_users_show_emails_requires_the_explicit_flag() {
    let pool = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let default = run(&pool, &arguments(InspectKind::Users, false, false)).await;
    assert!(!default.stdout.contains("alice.buyer@example.test"));

    let shown = run(&pool, &arguments(InspectKind::Users, false, true)).await;
    assert_eq!(shown.exit, Exit::Ok);
    assert!(
        shown
            .stdout
            .contains(" - alice.buyer@example.test (Role: UserRole.BUYER)")
    );
    // The stored value is shown unchanged, including the leading space of the
    // duplicate pair, and a user without a role prints None.
    assert!(
        shown
            .stdout
            .contains(" -  dup@example.test (Role: UserRole.ADMIN)")
    );
    assert!(shown.stdout.contains(" - grace@example.test (Role: None)"));
    // The summary line never carries an address, even with the flag.
    assert!(!shown.stderr.contains("example.test"), "{}", shown.stderr);

    let json = run(&pool, &arguments(InspectKind::Users, true, true)).await;
    let report: Value = serde_json::from_str(&json.stdout).expect("one JSON object");
    assert_eq!(report["emails"], "full");
    assert_eq!(report["total"], 12);
    assert_eq!(report["truncated"], false);
}

/// The ` - ` lines of the Python users tool, sorted (it prints in table order).
fn python_user_lines(report: &str) -> Vec<String> {
    let mut lines: Vec<String> = report
        .lines()
        .filter(|line| line.starts_with(" - "))
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

#[tokio::test]
async fn inspect_users_matches_fixed_rows() {
    let pool = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let rust = run(&pool, &arguments(InspectKind::Users, false, true)).await;
    assert_eq!(rust.exit, Exit::Ok);
    // Same header, and the same user lines (full addresses, the Python role
    // spelling, `None` for a missing role). Python lists in table order and
    // Rust in a fixed order, so the lines are compared sorted.
    let rust_lines = python_user_lines(&rust.stdout);
    assert_eq!(rust_lines.len(), 12);
    // A stored role that is not a role of the Python model makes Python fail
    // on load; Rust prints the stored value after the same prefix. That
    // difference is accepted: the column holds only model values.
}

// --- The commands only read --------------------------------------------------

async fn directory_listing(path: &std::path::Path) -> Vec<(String, u64)> {
    let mut entries: Vec<(String, u64)> = std::fs::read_dir(path)
        .map(|iterator| {
            iterator
                .flatten()
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

async fn row_fingerprint(connection: &mut PgConnection) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM users), \
                (SELECT count(*) FROM inspect_write_probe), \
                (SELECT count(*) FROM orderbook_orders)",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("count rows")
}

#[tokio::test]
async fn inspect_users_is_read_only_and_never_writes_a_status_file() {
    let owner = url("RUST_INSPECT_OWNER_DATABASE_URL");
    let mut observer = PgConnection::connect(&owner)
        .await
        .expect("owner connection");

    // 1. A writer role: the probe function writes in an ordinary transaction
    //    (rolled back here), so the refusal below comes from the transaction.
    let mut writable = PgConnection::connect(&owner)
        .await
        .expect("owner connection");
    let mut ordinary = sqlx::Connection::begin(&mut writable).await.expect("begin");
    let written: i32 = sqlx::query_scalar("SELECT inspect_write_probe_run()")
        .fetch_one(&mut *ordinary)
        .await
        .expect("the owner can write outside a read-only transaction");
    assert_eq!(written, 1);
    ordinary.rollback().await.expect("rollback");

    // 2. The inspection transaction refuses the same write: SQLSTATE 25006.
    let owner_pool = pool("RUST_INSPECT_OWNER_DATABASE_URL").await;
    let mut inspection = begin_read_only(&owner_pool)
        .await
        .expect("read-only transaction");
    let refusal = sqlx::query_scalar::<_, i32>("SELECT inspect_write_probe_run()")
        .fetch_one(&mut *inspection)
        .await
        .expect_err("a write inside the inspection transaction must fail");
    assert_eq!(
        refusal
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("25006"),
        "{refusal:?}"
    );
    inspection.rollback().await.expect("rollback");

    // 3. The settings the proof depends on.
    let mut inspection = begin_read_only(&owner_pool)
        .await
        .expect("read-only transaction");
    let flag: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *inspection)
        .await
        .expect("show");
    let level: String = sqlx::query_scalar("SHOW transaction_isolation")
        .fetch_one(&mut *inspection)
        .await
        .expect("show");
    assert_eq!((flag.as_str(), level.as_str()), ("on", "repeatable read"));

    // 4. Both reports on that transaction assign no transaction id, which a
    //    write of any kind would.
    collect_users(&mut inspection, false).await.expect("users");
    collect_blockers(&mut inspection).await.expect("blockers");
    let assigned: bool = sqlx::query_scalar("SELECT txid_current_if_assigned() IS NOT NULL")
        .fetch_one(&mut *inspection)
        .await
        .expect("xid check");
    assert!(!assigned, "the inspection assigned a transaction id");
    inspection.rollback().await.expect("rollback");

    // 5. Full commands under the writer role change no row and leave no file.
    let before_rows = row_fingerprint(&mut observer).await;
    let working = std::env::current_dir().expect("cwd");
    let before_files = directory_listing(&working).await;
    for (kind, json, show) in [
        (InspectKind::Users, false, false),
        (InspectKind::Users, true, true),
        (InspectKind::Blockers, false, false),
        (InspectKind::Blockers, true, false),
    ] {
        let result = run(&owner_pool, &arguments(kind, json, show)).await;
        assert!(
            matches!(result.exit, Exit::Ok | Exit::Usage),
            "{:?}",
            result.exit
        );
    }
    assert_eq!(row_fingerprint(&mut observer).await, before_rows);
    assert_eq!(
        directory_listing(&working).await,
        before_files,
        "no status file or other file may appear in the working directory"
    );
}

// --- Counts against the Python report ---------------------------------------

#[tokio::test]
async fn inspect_blockers_matches_fixed_counts() {
    let pool = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let rust = run(&pool, &arguments(InspectKind::Blockers, false, false)).await;

    assert_eq!(rust.exit, Exit::Usage);

    // The expected numbers are written in tests/fixtures/phase10_inspect_seed.sql.
    let expected = values(&rust.stdout);
    for (name, count) in [
        ("ineligible_users", "3"),
        ("duplicate_case_insensitive_emails", "1"),
        ("legacy_orders_without_owner", "5"),
        ("outstanding_legacy_orders_without_owner", "2"),
        ("legacy_rfqs_without_owner", "5"),
        ("outstanding_legacy_rfqs_without_owner", "2"),
        ("legacy_quotes_without_owner", "5"),
        ("outstanding_legacy_quotes_without_owner", "2"),
        ("legacy_trades_without_user_provenance", "8"),
        ("outstanding_legacy_trades_without_user_provenance", "4"),
        ("legacy_negotiations_without_user_provenance", "6"),
        (
            "outstanding_legacy_negotiations_without_user_provenance",
            "3",
        ),
        ("legacy_negotiation_rounds_without_user_provenance", "2"),
        ("approved_kyc_without_external_review_evidence", "2"),
        ("pending_organization_join_reviews", "2"),
        ("enforcement_preflight", "BLOCKED"),
        ("kyc_enforcement", "ADVISORY_HELD"),
    ] {
        assert_eq!(expected[name], count, "{name}");
    }
    assert_eq!(user_lines(&rust.stdout).len(), 3);
    // No address and no token reaches either stream.
    assert!(!rust.stdout.contains("example.test"));
    assert!(!rust.stderr.contains("example.test"));
}

#[tokio::test]
async fn inspect_blockers_exits_2_when_any_count_is_nonzero() {
    let blocked = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let result = run(&blocked, &arguments(InspectKind::Blockers, false, false)).await;
    assert_eq!(result.exit, Exit::Usage);
    assert_eq!(result.exit as i32, 2);
    let summary: Value = serde_json::from_str(result.stderr.trim()).expect("summary line");
    assert_eq!(summary["outcome"], "blocked");

    let clean = pool("RUST_INSPECT_CLEAN_DATABASE_URL").await;
    let ready = run(&clean, &arguments(InspectKind::Blockers, false, false)).await;
    assert_eq!(ready.exit, Exit::Ok);
    assert_eq!(values(&ready.stdout)["enforcement_preflight"], "READY");
    assert_eq!(values(&ready.stdout)["ineligible_users"], "0");
    let summary: Value = serde_json::from_str(ready.stderr.trim()).expect("summary line");
    assert_eq!(summary["outcome"], "ok");
}

#[tokio::test]
async fn inspect_json_is_one_object_with_stable_keys_on_a_real_database() {
    let pool = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let result = run(&pool, &arguments(InspectKind::Blockers, true, false)).await;
    assert_eq!(result.exit, Exit::Usage);
    assert_eq!(result.stdout.matches('\n').count(), 1, "one line");
    let report: Value = serde_json::from_str(&result.stdout).expect("one JSON object");
    assert_eq!(report["enforcement_preflight"], "BLOCKED");
    assert_eq!(report["counts"]["ineligible_users"], 3);
    assert_eq!(
        report["counts"]["outstanding_legacy_trades_without_user_provenance"],
        4
    );
    assert_eq!(
        report["ineligible_users"].as_array().expect("list").len(),
        3
    );
    assert_eq!(report["ineligible_users_truncated"], false);
    assert_eq!(
        report["ineligible_users"][1]["organization_id"],
        Value::Null
    );
    assert_eq!(report["ineligible_users"][0]["email_verified"], false);
    assert!(!result.stdout.contains("example.test"));
}

#[tokio::test]
async fn the_reader_role_is_enough_for_both_commands() {
    // The reader holds SELECT only; a command that needed more would fail here.
    let pool = pool("RUST_INSPECT_TEST_DATABASE_URL").await;
    let identity: (String,) = sqlx::query_as("SELECT current_user")
        .fetch_one(&pool)
        .await
        .expect("identity");
    assert_eq!(identity.0, "inspect_reader");
    for kind in [InspectKind::Users, InspectKind::Blockers] {
        let result = run(&pool, &arguments(kind, true, false)).await;
        assert!(matches!(result.exit, Exit::Ok | Exit::Usage));
    }
}

//! Phase 10 Task 5: the frozen sqlx baseline (`rs/read/migrations/1_baseline.sql`)
//! and its manifest (`deploy/rust/sqlx-baseline.json`).
//!
//! The database cases restore the committed baseline into fresh databases on
//! the disposable PostgreSQL of `run_postgres.sh`, as the production migrator
//! role name, so the owner projection of the catalog fingerprint matches the
//! manifest. The fingerprint is the SHA-256 of the lines of
//! `deploy/rust/catalog-fingerprint.sql` joined by "\n", read in one
//! REPEATABLE READ transaction.
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};
use std::collections::BTreeMap;
use std::str::FromStr;

const BASELINE_SQL: &str = include_str!("../migrations/1_baseline.sql");
const MANIFEST: &str = include_str!("../../../deploy/rust/sqlx-baseline.json");
const FINGERPRINT_SQL: &str = include_str!("../../../deploy/rust/catalog-fingerprint.sql");
const HANDOVER: &str = include_str!("../../../deploy/rust/schema-handover.json");
const ALLOWED_REVISIONS: &str = include_str!("../../../deploy/rust/allowed-revisions.json");
const CHECKPOINTS: &str = include_str!("../../../deploy/migration-checkpoints.tsv");
/// The production migrator role: the baseline's objects belong to it.
const MIGRATOR_ROLE: &str = "verdaxis_migrator";

struct Manifest {
    file_sha256: String,
    restored_fingerprint: String,
    reference_rows: BTreeMap<String, i64>,
}

fn manifest() -> Manifest {
    let document: serde_json::Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    let text = |key: &str| {
        document[key]
            .as_str()
            .unwrap_or_else(|| panic!("manifest {key}"))
            .to_owned()
    };
    let reference_rows = document["reference_rows"]
        .as_object()
        .expect("manifest reference_rows")
        .iter()
        .map(|(table, count)| (table.clone(), count.as_i64().expect("row count")))
        .collect();
    Manifest {
        file_sha256: text("file_sha256"),
        restored_fingerprint: text("restored_fingerprint"),
        reference_rows,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn admin_url() -> String {
    std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
        .expect("RUST_TEMPLATE_ADMIN_DATABASE_URL must name the disposable server")
}

/// A connection to `database` on the disposable server, as the administrator.
async fn connect(database: &str) -> PgConnection {
    let options = PgConnectOptions::from_str(&admin_url())
        .expect("parse the administrator URL")
        .database(database);
    PgConnection::connect_with(&options)
        .await
        .expect("connect to the disposable server")
}

/// A fresh database owned by the migrator role, with PostGIS created by the
/// administrator before any migration, as on the deployed hosts.
async fn fresh_database(label: &str) -> String {
    let name = format!(
        "verdaxis_baseline_{label}_{}",
        uuid::Uuid::new_v4().simple()
    );
    let mut admin = connect("postgres").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{MIGRATOR_ROLE}') THEN \
             CREATE ROLE {MIGRATOR_ROLE} NOLOGIN; \
           END IF; \
         END $$"
    )))
    .execute(&mut admin)
    .await
    .expect("the migrator role");
    // CREATE DATABASE cannot share a statement batch: a batch is one transaction.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER {MIGRATOR_ROLE}"
    )))
    .execute(&mut admin)
    .await
    .expect("create a fresh database");
    let mut database = connect(&name).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE EXTENSION IF NOT EXISTS postgis; ALTER SCHEMA public OWNER TO {MIGRATOR_ROLE};"
    )))
    .execute(&mut database)
    .await
    .expect("prepare the fresh database");
    name
}

/// Run the committed baseline in `database` as the migrator role.
async fn restore(database: &str) -> Result<(), sqlx::Error> {
    let mut connection = connect(database).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET ROLE {MIGRATOR_ROLE}")))
        .execute(&mut connection)
        .await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(BASELINE_SQL))
        .execute(&mut connection)
        .await?;
    Ok(())
}

async fn fingerprint(database: &str) -> String {
    let mut connection = connect(database).await;
    let mut transaction = connection.begin().await.expect("begin");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await
        .expect("repeatable read");
    let lines: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(FINGERPRINT_SQL))
        .fetch_all(&mut *transaction)
        .await
        .expect("catalog projection");
    transaction.commit().await.expect("commit");
    sha256_hex(lines.join("\n").as_bytes())
}

/// Row counts of every ordinary public table, extension members excluded.
async fn row_counts(database: &str) -> BTreeMap<String, i64> {
    let mut connection = connect(database).await;
    let rows = sqlx::query(
        "SELECT c.relname, \
                (xpath('/row/n/text()', query_to_xml(format('SELECT count(*) AS n FROM %I.%I', \
                  n.nspname, c.relname), false, true, '')))[1]::text::bigint AS n \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') \
           AND c.relname NOT IN ('alembic_version', '_sqlx_migrations') \
           AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.deptype = 'e' \
             AND d.classid = 'pg_class'::regclass AND d.objid = c.oid)",
    )
    .fetch_all(&mut connection)
    .await
    .expect("row counts");
    rows.iter()
        .map(|row| (row.get::<String, _>(0), row.get::<i64, _>(1)))
        .collect()
}

async fn drop_database(name: &str) {
    let mut admin = connect("postgres").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
    )))
    .execute(&mut admin)
    .await
    .expect("drop the case database");
}

#[test]
fn baseline_dump_has_no_owner_or_privilege_clauses() {
    for (number, line) in BASELINE_SQL.lines().enumerate() {
        let upper = line.trim_start().to_ascii_uppercase();
        // Comments (also inside function bodies) are text, not clauses.
        if upper.starts_with("--") {
            continue;
        }
        for clause in ["OWNER TO", "GRANT ", "REVOKE ", "ALTER DEFAULT PRIVILEGES"] {
            assert!(
                !(upper.starts_with(clause) || upper.contains(&format!(" {clause}"))),
                "line {}: {clause} in the baseline",
                number + 1
            );
        }
        // sqlx runs the file as plain SQL: no psql command may remain.
        assert!(
            !line.starts_with('\\'),
            "line {}: psql command in the baseline",
            number + 1
        );
    }
    assert_eq!(
        sha256_hex(BASELINE_SQL.as_bytes()),
        manifest().file_sha256,
        "the baseline file differs from the digest in the manifest"
    );
}

/// The producer register names the head the baseline was built from. That
/// head is the only head of the Alembic graph in this tree, the newest
/// revision the Rust service accepts, and the register's approved transitions
/// are exactly the checkpoint-policy rows that end at it.
#[test]
fn handover_register_is_closed_at_the_baseline_head() {
    let handover: serde_json::Value = serde_json::from_str(HANDOVER).expect("handover JSON");
    let baseline: serde_json::Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    assert_eq!(
        handover["status"], "producers-closed",
        "the producer register is open"
    );
    let final_head = handover["final_head"].as_str().expect("final head");
    assert_eq!(final_head, baseline["alembic_head"]);
    assert_eq!(handover["alembic_revisions"], baseline["alembic_revisions"]);

    let (revision_count, heads) = alembic_graph_heads();
    assert_eq!(heads, [final_head.to_owned()], "the Alembic graph heads");
    assert_eq!(
        handover["alembic_revisions"].as_u64(),
        Some(revision_count as u64)
    );

    let allowed: Vec<String> = serde_json::from_str(ALLOWED_REVISIONS).expect("allowed revisions");
    assert_eq!(
        allowed.last().map(String::as_str),
        Some(final_head),
        "the final head is not the newest revision the Rust service accepts"
    );

    let mut policy_to_head: Vec<(String, String)> = CHECKPOINTS
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, to)| *to == final_head)
        .map(|(from, to)| (from.to_owned(), to.to_owned()))
        .collect();
    let mut approved: Vec<(String, String)> = handover["approved_transitions_to_final_head"]
        .as_array()
        .expect("approved transitions")
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().expect("from revision").to_owned(),
                pair[1].as_str().expect("to revision").to_owned(),
            )
        })
        .collect();
    policy_to_head.sort();
    approved.sort();
    assert!(!approved.is_empty(), "the register approves no transition");
    assert_eq!(
        approved, policy_to_head,
        "the register's transitions differ from the checkpoint policy rows that end at the head"
    );
}

/// Every quoted name in one Python assignment such as `revision = "a"` or
/// `down_revision = ("a", "b")`, which may continue over several lines.
fn assigned_names(source: &str, name: &str) -> Vec<String> {
    let Some(start) = source.lines().position(|line| {
        line.starts_with(&format!("{name} ")) || line.starts_with(&format!("{name}:"))
    }) else {
        return Vec::new();
    };
    let mut text = String::new();
    for line in source.lines().skip(start) {
        text.push_str(line);
        text.push('\n');
        let opened = text.matches('(').count();
        if opened == text.matches(')').count() {
            break;
        }
    }
    let value = text.split_once('=').map(|(_, value)| value).unwrap_or("");
    value
        .split(['"', '\''])
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The revision count and the heads of the Alembic graph in `alembic/versions`:
/// revisions that no other revision names as a parent.
fn alembic_graph_heads() -> (usize, Vec<String>) {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../alembic/versions");
    let mut revisions = Vec::new();
    let mut parents = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(folder).expect("alembic/versions") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("py") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("revision file");
        let names = assigned_names(&source, "revision");
        let [revision] = names.as_slice() else {
            panic!("{}: expected one revision name", path.display());
        };
        revisions.push(revision.clone());
        parents.extend(assigned_names(&source, "down_revision"));
    }
    let heads = revisions
        .iter()
        .filter(|revision| !parents.contains(*revision))
        .cloned()
        .collect();
    (revisions.len(), heads)
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn baseline_fingerprint_matches_the_committed_artifact() {
    let database = fresh_database("fingerprint").await;
    restore(&database).await.expect("restore the baseline");
    assert_eq!(
        fingerprint(&database).await,
        manifest().restored_fingerprint
    );
    drop_database(&database).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn baseline_contains_reference_rows_and_no_application_rows() {
    let database = fresh_database("rows").await;
    restore(&database).await.expect("restore the baseline");
    let counts = row_counts(&database).await;
    let expected = manifest().reference_rows;
    for (table, count) in &counts {
        let reference = expected.get(table).copied().unwrap_or(0);
        assert_eq!(*count, reference, "{table}: rows after the baseline");
    }
    for table in expected.keys() {
        assert!(counts.contains_key(table), "{table} is not in the baseline");
    }
    drop_database(&database).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn baseline_restores_into_an_empty_database() {
    let first = fresh_database("first").await;
    let second = fresh_database("second").await;
    restore(&first)
        .await
        .expect("restore into the first database");
    restore(&second)
        .await
        .expect("restore into the second database");
    assert_eq!(fingerprint(&first).await, fingerprint(&second).await);

    // Each database gets its own identity: the baseline keeps the migration's
    // gen_random_uuid() instead of a frozen value.
    let identity = |name: String| async move {
        let mut connection = connect(&name).await;
        sqlx::query_scalar::<_, String>(
            "SELECT database_epoch || '/' || primary_identity FROM public_read_cache_incarnation",
        )
        .fetch_one(&mut connection)
        .await
        .expect("cache incarnation")
    };
    assert_ne!(
        identity(first.clone()).await,
        identity(second.clone()).await
    );

    // The baseline is not re-runnable: adoption is the only path for a
    // database that already has the schema.
    assert!(
        restore(&first).await.is_err(),
        "a second restore over the baseline was accepted"
    );
    drop_database(&first).await;
    drop_database(&second).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn fingerprint_is_stable_and_changes_on_a_new_column() {
    let database = fresh_database("change").await;
    restore(&database).await.expect("restore the baseline");
    let before = fingerprint(&database).await;
    assert_eq!(fingerprint(&database).await, before, "two reads differ");

    // ACLs and owners are outside the projection (plan section 8.3).
    let mut admin = connect(&database).await;
    sqlx::raw_sql(
        "CREATE ROLE verdaxis_fingerprint_probe NOLOGIN; \
         GRANT SELECT ON public.products TO verdaxis_fingerprint_probe; \
         REVOKE SELECT ON public.products FROM verdaxis_fingerprint_probe; \
         DROP ROLE verdaxis_fingerprint_probe;",
    )
    .execute(&mut admin)
    .await
    .expect("grant and revoke on a probe role");
    assert_eq!(
        fingerprint(&database).await,
        before,
        "an ACL change moved it"
    );

    sqlx::raw_sql("ALTER TABLE public.products ADD COLUMN fingerprint_probe integer")
        .execute(&mut admin)
        .await
        .expect("add a column");
    assert_ne!(
        fingerprint(&database).await,
        before,
        "a new column did not move it"
    );
    drop_database(&database).await;
}

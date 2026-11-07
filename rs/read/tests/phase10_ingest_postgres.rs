//! Phase 10 Task 21: `ops ingest signals` against the disposable PostgreSQL of
//! `run_postgres.sh`. Each case clones `RUST_INGEST_TEMPLATE_DATABASE` (the
//! baseline applied by the staging migrator, then the real ACL convergence
//! with the operator role) as `verdaxis_staging`, the only database the command
//! accepts, and runs the real binary as `verdaxis_operator_staging`. The cases
//! run one at a time and drop the clone.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OPERATOR: &str = "verdaxis_operator_staging";
const OPERATOR_PASSWORD: &str = "disposable_operator_pw";
const APP: &str = "verdaxis_app_staging";
const APP_PASSWORD: &str = "disposable_staging_pw";
const DATABASE: &str = "verdaxis_staging";
const INDICATIONS: &str = "source_event_id,market_product,delivery_point,availability_window,side,price_per_mt_usd,quantity_mt,observed_at";

fn admin_options() -> PgConnectOptions {
    let url = std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
        .expect("RUST_TEMPLATE_ADMIN_DATABASE_URL must name the disposable server");
    PgConnectOptions::from_str(&url).expect("parse the administrator URL")
}

async fn admin(database: &str, statement: &str) {
    let mut connection = PgConnection::connect_with(&admin_options().database(database))
        .await
        .unwrap();
    // Fixed test statements, written in this file.
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

/// A fresh `verdaxis_staging` cloned from the converged template.
async fn fresh_staging() {
    let template = std::env::var("RUST_INGEST_TEMPLATE_DATABASE")
        .expect("RUST_INGEST_TEMPLATE_DATABASE names the converged template");
    admin(
        "postgres",
        &format!("DROP DATABASE IF EXISTS {DATABASE} WITH (FORCE)"),
    )
    .await;
    admin(
        "postgres",
        &format!("CREATE DATABASE {DATABASE} TEMPLATE {template}"),
    )
    .await;
}

async fn connect_as(user: &str, password: &str) -> PgConnection {
    let options = admin_options()
        .username(user)
        .password(password)
        .database(DATABASE);
    PgConnection::connect_with(&options).await.unwrap()
}

fn operator_url() -> String {
    let options = admin_options();
    format!(
        "postgres://{OPERATOR}:{OPERATOR_PASSWORD}@{}:{}/{DATABASE}",
        options.get_host(),
        options.get_port()
    )
}

fn csv_file(name: &str, rows: &[&str]) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("verdaxis-ingest-{}-{name}.csv", std::process::id()));
    let mut text = format!("{INDICATIONS}\n");
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
    path
}

fn ingest(file: &Path, apply: bool, url: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ENVIRONMENT", "staging")
        .env("RELEASE_SHA", SHA)
        .env("OPERATOR_DATABASE_URL", url)
        .args([
            "ops",
            "ingest",
            "signals",
            "--family",
            "indications",
            "--source",
            "desk-marks",
        ])
        .arg("--file")
        .arg(file)
        .args(["--environment", "staging", "--release-sha", SHA]);
    if apply {
        command.arg("--apply");
    }
    command.output().unwrap()
}

fn report(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "no report: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

async fn count(sql: &str) -> i64 {
    let mut connection = PgConnection::connect_with(&admin_options().database(DATABASE))
        .await
        .unwrap();
    let value: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value
}

/// A recent observation, so no staleness warning; Singapore takes B30.
fn row(event: &str, price: &str) -> String {
    let observed = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    format!("{event},B30,Singapore,SPOT,BID,{price},100,{observed}")
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn signal_dry_run_writes_nothing_and_reports_would_insert() {
    fresh_staging().await;
    let file = csv_file("dry", &[&row("e1", "700.004"), &row("e2", "701")]);
    let output = ingest(&file, false, &operator_url());
    assert_eq!(output.status.code(), Some(0));
    let report = report(&output);
    assert_eq!(report["mode"], "dry_run");
    assert_eq!(report["would_insert"], 2);
    assert_eq!(report["inserted"], 0);
    assert!(report["run_id"].is_null());
    assert_eq!(count("SELECT count(*) FROM market_indications").await, 0);
    assert_eq!(
        count("SELECT count(*) FROM market_signal_ingestion_runs").await,
        0
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn signal_row_error_rolls_back_only_that_row() {
    fresh_staging().await;
    let file = csv_file(
        "row-error",
        &[&row("e1", "700.005"), &row("e2", "-5"), &row("e3", "702")],
    );
    let output = ingest(&file, true, &operator_url());
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(&output);
    assert_eq!(report["inserted"], 2);
    assert_eq!(
        report["errors"],
        serde_json::json!([{"row": 2, "reason": "price_per_mt_usd must be positive"}])
    );
    let run = report["run_id"].as_str().unwrap().to_owned();
    assert_eq!(
        count(&format!(
            "SELECT count(*) FROM market_indications WHERE trusted_ingestion_run_id = '{run}' \
             AND is_verified_real AND NOT is_demo AND verified_real_at IS NOT NULL"
        ))
        .await,
        2
    );
    assert_eq!(
        count(&format!(
            "SELECT count(*) FROM market_signal_ingestion_runs WHERE id = '{run}' \
             AND verified_at IS NOT NULL AND signal_family = 'MARKET_INDICATION' \
             AND source_kind = 'MARKET_INDICATION' AND source = 'desk-marks'"
        ))
        .await,
        1
    );
    // Prices are rounded half-even to cents, as Python quantized them.
    assert_eq!(
        count("SELECT count(*) FROM market_indications WHERE source_event_id = 'e1' AND price_per_mt_usd = 700.00").await,
        1
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn signal_duplicate_source_event_id_is_skipped_not_failed() {
    fresh_staging().await;
    let first = csv_file("first", &[&row("e1", "700"), &row("e2", "701")]);
    assert_eq!(ingest(&first, true, &operator_url()).status.code(), Some(0));
    // The same file again: every row is a duplicate, and no empty run is kept.
    let again = ingest(&first, true, &operator_url());
    assert_eq!(again.status.code(), Some(0));
    let report_again = report(&again);
    assert_eq!(
        (
            report_again["inserted"].as_u64(),
            report_again["skipped_duplicates"].as_u64()
        ),
        (Some(0), Some(2))
    );
    assert!(report_again["run_id"].is_null());
    assert_eq!(
        count("SELECT count(*) FROM market_signal_ingestion_runs").await,
        1
    );
    // One new row and one duplicate.
    let mixed = csv_file("mixed", &[&row("e2", "705"), &row("e3", "706")]);
    let report_mixed = report(&ingest(&mixed, true, &operator_url()));
    assert_eq!(
        (
            report_mixed["inserted"].as_u64(),
            report_mixed["skipped_duplicates"].as_u64()
        ),
        (Some(1), Some(1))
    );
    assert_eq!(count("SELECT count(*) FROM market_indications").await, 3);
    assert_eq!(
        count("SELECT count(*) FROM market_signal_ingestion_runs").await,
        2
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn signal_ingest_uses_operator_role_and_app_role_is_denied() {
    fresh_staging().await;
    let mut app = connect_as(APP, APP_PASSWORD).await;
    for statement in [
        "INSERT INTO market_signal_ingestion_runs (id, signal_family, source, source_kind) \
         VALUES (gen_random_uuid(), 'MARKET_INDICATION', 'desk-marks', 'MARKET_INDICATION')",
        "DELETE FROM market_indications",
    ] {
        let error = sqlx::raw_sql(statement)
            .execute(&mut app)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("permission denied"),
            "{statement}: {error}"
        );
    }
    // The command refuses an app-role URL before connecting.
    let file = csv_file("app-role", &[&row("e1", "700")]);
    let app_url = operator_url().replace(
        &format!("{OPERATOR}:{OPERATOR_PASSWORD}"),
        &format!("{APP}:{APP_PASSWORD}"),
    );
    assert_eq!(ingest(&file, true, &app_url).status.code(), Some(3));
    assert_eq!(count("SELECT count(*) FROM market_indications").await, 0);
    // The operator writes, and the run names it.
    assert_eq!(ingest(&file, true, &operator_url()).status.code(), Some(0));
    assert_eq!(count("SELECT count(*) FROM market_indications").await, 1);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn operator_role_is_exact_and_not_a_member_of_the_app_role() {
    fresh_staging().await;
    let mut connection = PgConnection::connect_with(&admin_options().database(DATABASE))
        .await
        .unwrap();
    let role = sqlx::query(
        "SELECT rolcanlogin, rolsuper, rolinherit, rolcreatedb, rolcreaterole, rolreplication, \
         rolbypassrls, EXISTS (SELECT 1 FROM pg_auth_members WHERE member = r.oid OR roleid = r.oid) \
         FROM pg_roles AS r WHERE rolname = $1",
    )
    .bind(OPERATOR)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let flags: Vec<bool> = (0..8).map(|index| role.get(index)).collect();
    assert_eq!(
        flags,
        [true, false, false, false, false, false, false, false]
    );
    // Convergence refuses an operator that gains a membership.
    connection.close().await.unwrap();
    admin("postgres", &format!("GRANT {APP} TO {OPERATOR}")).await;
    let refused = converge_operator();
    admin("postgres", &format!("REVOKE {APP} FROM {OPERATOR}")).await;
    assert!(
        !refused,
        "convergence must refuse an operator that is a member of the app role"
    );
}

/// Run the committed convergence on the clone through the container's psql,
/// as the harness does; true when it succeeds.
fn converge_operator() -> bool {
    let container = std::env::var("RUST_POSTGRES_CONTAINER").expect("RUST_POSTGRES_CONTAINER");
    Command::new("docker")
        .args([
            "exec", &container, "psql", "-U", "postgres", "-d", DATABASE, "-X", "-q",
        ])
        .args(["--single-transaction", "-v", "ON_ERROR_STOP=1"])
        .args([
            "-v",
            &format!("database_name={DATABASE}"),
            "-v",
            &format!("app_role={APP}"),
        ])
        .args([
            "-v",
            "migrator_role=verdaxis_migrator_staging",
            "-v",
            "backup_role=verdaxis_backup_staging",
        ])
        .args([
            "-v",
            &format!("operator_role={OPERATOR}"),
            "-f",
            "/tmp/acl-bundle/converge_runtime_object_acls.sql",
        ])
        .output()
        .unwrap()
        .status
        .success()
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn operator_acl_grants_only_ingestion_objects() {
    fresh_staging().await;
    let expected = [
        ("delivery_points", "SELECT"),
        ("fair_price_bands", "INSERT,SELECT"),
        ("market_indications", "INSERT,SELECT"),
        (
            "market_signal_ingestion_runs",
            "DELETE,INSERT,SELECT,UPDATE",
        ),
        ("physical_stems", "INSERT,SELECT"),
        ("ports", "SELECT"),
        // Task 22: the GENA producer-project import.
        ("producer_projects", "INSERT,SELECT"),
        ("products", "SELECT"),
    ];
    let grants = || async {
        let mut connection = PgConnection::connect_with(&admin_options().database(DATABASE))
            .await
            .unwrap();
        // Table and sequence grants of the operator, then any column grant.
        let tables: Vec<(String, String)> = sqlx::query_as(
            "SELECT c.relname::text, string_agg(a.privilege_type, ',' ORDER BY a.privilege_type) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             CROSS JOIN LATERAL aclexplode(c.relacl) a \
             WHERE n.nspname = 'public' AND a.grantee = $1::regrole \
             GROUP BY c.relname ORDER BY c.relname",
        )
        .bind(OPERATOR)
        .fetch_all(&mut connection)
        .await
        .unwrap();
        let columns: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_attribute att JOIN pg_class c ON c.oid = att.attrelid \
             CROSS JOIN LATERAL aclexplode(att.attacl) a WHERE a.grantee = $1::regrole",
        )
        .bind(OPERATOR)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        (tables, columns)
    };
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(table, privileges)| ((*table).to_owned(), (*privileges).to_owned()))
        .collect();
    assert_eq!(grants().await, (expected.clone(), 0));
    // A second convergence changes nothing.
    assert!(converge_operator());
    assert_eq!(grants().await, (expected, 0));
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn signal_output_contains_counts_but_no_row_contents() {
    fresh_staging().await;
    let observed = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let file = csv_file(
        "secrets",
        &[
            &format!("SECRET-EVENT-1,B30,Singapore,SPOT,BID,SECRET-PRICE,100,{observed}"),
            &format!("e2,B30,SECRET-PORT,SPOT,BID,700,100,{observed}"),
            &format!("e3,SECRET-PRODUCT,Singapore,SPOT,BID,700,100,{observed}"),
            &row("e4", "700"),
        ],
    );
    for apply in [false, true] {
        let output = ingest(&file, apply, &operator_url());
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("SECRET"), "a row value was printed: {text}");
        let report = report(&output);
        assert_eq!(report["errors"].as_array().unwrap().len(), 3);
    }
}

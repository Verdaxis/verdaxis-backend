//! Phase 10 Task 22: `ops import gena` against the disposable PostgreSQL of
//! `run_postgres.sh`. Each case clones `RUST_INGEST_TEMPLATE_DATABASE` (the
//! baseline applied by the staging migrator, then the real ACL convergence
//! with the operator role) as `verdaxis_staging` and runs the real binary as
//! `verdaxis_operator_staging`. The cases run one at a time.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::str::FromStr;

use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OPERATOR: &str = "verdaxis_operator_staging";
const OPERATOR_PASSWORD: &str = "disposable_operator_pw";
const APP: &str = "verdaxis_app_staging";
const APP_PASSWORD: &str = "disposable_staging_pw";
const DATABASE: &str = "verdaxis_staging";
const HEADER: &str =
    "project_name,country,region,capacity_kt,cod_year,status,feedstock,technology,lat,lng,gena_id";

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
    // The case counts only its own rows.
    admin(DATABASE, "DELETE FROM producer_projects").await;
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
        std::env::temp_dir().join(format!("verdaxis-gena-{}-{name}.csv", std::process::id()));
    let mut text = format!("{HEADER}\n");
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
    path
}

fn import_command(file: &Path, extra: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ENVIRONMENT", "staging")
        .env("RELEASE_SHA", SHA)
        .env("OPERATOR_DATABASE_URL", operator_url())
        .args(["ops", "import", "gena", "--fuel-type", "Methanol"])
        .arg("--file")
        .arg(file)
        .args(["--environment", "staging", "--release-sha", SHA])
        .args(extra);
    command
}

fn import(file: &Path, extra: &[&str]) -> Output {
    import_command(file, extra).output().unwrap()
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

async fn scalar<T>(sql: &str) -> T
where
    T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
{
    let mut connection = PgConnection::connect_with(&admin_options().database(DATABASE))
        .await
        .unwrap();
    let value = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value
}

const ROW_ONE: &str = "Green Methanol One,Denmark,Europe,100.5,2027,operational,biomass,gasification,55.6,12.5,GENA-1";

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_dry_run_writes_nothing() {
    fresh_staging().await;
    let file = csv_file("dry", &[ROW_ONE]);
    let output = import(&file, &[]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(report(&output)["would_import"], 1);
    assert_eq!(
        scalar::<i64>("SELECT count(*) FROM producer_projects").await,
        0
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_zero_coordinate_is_preserved_in_the_database() {
    fresh_staging().await;
    let file = csv_file("zero", &["Null Island Plant,Ghana,,,,,,,0,0,GENA-0"]);
    assert_eq!(import(&file, &["--apply"]).status.code(), Some(0));
    assert_eq!(
        scalar::<String>(
            "SELECT ST_AsText(location::geometry) FROM producer_projects WHERE gena_project_id = 'GENA-0'"
        )
        .await,
        "POINT(0 0)"
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_same_id_replay_inserts_once() {
    fresh_staging().await;
    let file = csv_file(
        "replay",
        &[ROW_ONE, "Second Plant,Chile,,,,announced,,,,,GENA-2"],
    );
    let first = report(&import(&file, &["--apply"]));
    assert_eq!(
        (
            first["created"].as_u64(),
            first["skipped_existing"].as_u64()
        ),
        (Some(2), Some(0))
    );
    let again = report(&import(&file, &["--apply"]));
    assert_eq!(
        (
            again["created"].as_u64(),
            again["skipped_existing"].as_u64()
        ),
        (Some(0), Some(2))
    );
    assert_eq!(
        scalar::<i64>("SELECT count(*) FROM producer_projects").await,
        2
    );
    assert_eq!(
        scalar::<String>(
            "SELECT status || '/' || fuel_type || '/' || data_source || '/' || capacity_kt_per_year::text \
             FROM producer_projects WHERE gena_project_id = 'GENA-1'"
        )
        .await,
        "OPERATIONAL/Methanol/GENA/100.50"
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_concurrent_import_keeps_one_row() {
    fresh_staging().await;
    let rows: Vec<String> = (1..=200)
        .map(|n| format!("Plant {n},Norway,,,,,,,,,GENA-{n}"))
        .collect();
    let row_refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let file = csv_file("concurrent", &row_refs);
    let children: Vec<_> = (0..4)
        .map(|_| {
            import_command(&file, &["--apply"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut created = 0;
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = report(&output);
        created += report["created"].as_u64().unwrap();
        assert_eq!(
            report["created"].as_u64().unwrap() + report["skipped_existing"].as_u64().unwrap(),
            200
        );
    }
    assert_eq!(created, 200, "every id is created by exactly one import");
    assert_eq!(
        scalar::<i64>("SELECT count(*) FROM producer_projects").await,
        200
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_allow_blank_id_is_explicit_and_counted() {
    fresh_staging().await;
    let file = csv_file("blank", &[ROW_ONE, "No Id Plant,Spain,,,,,,,,,"]);
    assert_eq!(import(&file, &["--apply"]).status.code(), Some(3));
    assert_eq!(
        scalar::<i64>("SELECT count(*) FROM producer_projects").await,
        0
    );
    let allowed = report(&import(&file, &["--apply", "--allow-blank-id"]));
    assert_eq!(
        (
            allowed["created"].as_u64(),
            allowed["created_without_id"].as_u64()
        ),
        (Some(1), Some(1))
    );
    // A blank id cannot be deduplicated: a replay inserts it again and says so.
    let replay = report(&import(&file, &["--apply", "--allow-blank-id"]));
    assert_eq!(
        (
            replay["created"].as_u64(),
            replay["skipped_existing"].as_u64(),
            replay["created_without_id"].as_u64()
        ),
        (Some(0), Some(1), Some(1))
    );
    assert_eq!(
        scalar::<i64>("SELECT count(*) FROM producer_projects WHERE gena_project_id IS NULL").await,
        2
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_invalid_row_leaves_no_partial_import() {
    fresh_staging().await;
    let file = csv_file("partial", &[ROW_ONE, "Bad Plant,Spain,,,,,,,95,10,GENA-3"]);
    assert_eq!(import(&file, &["--apply"]).status.code(), Some(3));
    assert_eq!(
        scalar::<i64>("SELECT count(*) FROM producer_projects").await,
        0
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn gena_operator_grant_is_exact_and_app_role_stays_read_only() {
    fresh_staging().await;
    assert_eq!(
        scalar::<String>(&format!(
            "SELECT string_agg(a.privilege_type, ',' ORDER BY a.privilege_type) FROM pg_class c \
             CROSS JOIN LATERAL aclexplode(c.relacl) a \
             WHERE c.relname = 'producer_projects' AND a.grantee = '{OPERATOR}'::regrole"
        ))
        .await,
        "INSERT,SELECT"
    );
    assert_eq!(
        scalar::<String>(&format!(
            "SELECT string_agg(a.privilege_type, ',' ORDER BY a.privilege_type) FROM pg_class c \
             CROSS JOIN LATERAL aclexplode(c.relacl) a \
             WHERE c.relname = 'producer_projects' AND a.grantee = '{APP}'::regrole"
        ))
        .await,
        "SELECT"
    );
    let options = admin_options()
        .username(APP)
        .password(APP_PASSWORD)
        .database(DATABASE);
    let mut app = PgConnection::connect_with(&options).await.unwrap();
    let error = sqlx::raw_sql(
        "INSERT INTO producer_projects (name, fuel_type, country) VALUES ('x', 'Methanol', 'Peru')",
    )
    .execute(&mut app)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("permission denied"), "{error}");
}

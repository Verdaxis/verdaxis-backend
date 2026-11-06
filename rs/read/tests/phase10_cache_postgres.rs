//! Phase 10 Task 24: `ops cache-authority activate` against the disposable
//! PostgreSQL of `run_postgres.sh`. Each case clones
//! `RUST_INGEST_TEMPLATE_DATABASE` (the baseline applied by the staging
//! migrator, then the real ACL convergence) as `verdaxis_staging` and runs the
//! real binary with a deployed-style environment file.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::str::FromStr;

use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use verdaxis_read::cli::ops::cache::{RECORD_NAME, Record, read_record};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const DATABASE: &str = "verdaxis_staging";
const MIGRATOR: (&str, &str) = ("verdaxis_migrator_staging", "disposable_migrator_pw");
const APP: (&str, &str) = ("verdaxis_app_staging", "disposable_staging_pw");

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
}

fn url(role: (&str, &str)) -> String {
    let options = admin_options();
    format!(
        "postgresql://{}:{}@{}:{}/{DATABASE}",
        role.0,
        role.1,
        options.get_host(),
        options.get_port()
    )
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-cache-pg-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("runtime")).unwrap();
        Self(path)
    }

    fn environment_file(&self, migrator_url: &str) -> PathBuf {
        let path = self.0.join(".env");
        std::fs::write(
            &path,
            format!("ENVIRONMENT=staging\nRELEASE_SHA={SHA}\nMIGRATOR_DATABASE_URL={migrator_url}\nJWT_SECRET=not-read\n"),
        )
        .unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn activate(runtime: &Path, environment_file: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["ops", "cache-authority", "activate", "--runtime-dir"])
        .arg(runtime)
        .arg("--environment-file")
        .arg(environment_file)
        .args(["--environment", "staging", "--release-sha", SHA, "--apply"])
        .output()
        .unwrap()
}

async fn incarnation() -> (String, String, i64, bool) {
    let mut connection = PgConnection::connect_with(&admin_options().database(DATABASE))
        .await
        .unwrap();
    let row = sqlx::query_as(
        "SELECT database_epoch, primary_identity, authority_revision, active \
         FROM public.public_read_cache_incarnation WHERE singleton",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    row
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn cache_activate_uses_the_migrator_role_only() {
    fresh_staging().await;
    let scratch = Scratch::new("migrator");
    let runtime = scratch.0.join("runtime");
    let before = incarnation().await;

    // The app role holds SELECT only: it cannot bind the row.
    let mut app = PgConnection::connect(&url(APP)).await.unwrap();
    let error = sqlx::query(
        "UPDATE public.public_read_cache_incarnation SET active = true WHERE singleton",
    )
    .execute(&mut app)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("permission denied"), "{error}");
    app.close().await.unwrap();

    // An app URL in the migrator slot is refused before any record or row change.
    let output = activate(&runtime, &scratch.environment_file(&url(APP)));
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!runtime.join(RECORD_NAME).exists());
    assert_eq!(incarnation().await, before);

    // The migrator binds the row, then the record names the same identity.
    let output = activate(&runtime, &scratch.environment_file(&url(MIGRATOR)));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let Some(Record {
        revision,
        authority: Some(authority),
    }) = read_record(&runtime).unwrap()
    else {
        panic!("an active record");
    };
    let row = incarnation().await;
    assert_eq!(
        row,
        (
            authority.database_epoch.clone(),
            authority.primary_identity.clone(),
            revision as i64,
            true
        )
    );
    assert_eq!(revision as i64, before.2 + 2, "revoked at +1, active at +2");
    // Output names the revision, never an identity.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(&authority.database_epoch)
            && !stdout.contains(&authority.primary_identity)
    );

    // A second activation rotates both identities and raises the revision.
    let output = activate(&runtime, &scratch.environment_file(&url(MIGRATOR)));
    assert_eq!(output.status.code(), Some(0));
    let rotated = incarnation().await;
    assert_eq!(rotated.2, revision as i64 + 2);
    assert_ne!(rotated.0, authority.database_epoch);
    assert_ne!(rotated.1, authority.primary_identity);
}

//! Phase 10 Task 9: `ops preflight` against the disposable PostgreSQL of
//! `run_postgres.sh`. The file checks are offline (`phase10_ops_env`); these
//! cases prove the one connection: the reported database and role, the role's
//! least privilege, and `max_connections` against the configured budget.
//!
//! The preflight accepts only the deployed names, so the case creates a
//! `verdaxis_staging` database on the disposable server and connects as the
//! harness's `verdaxis_app_staging` role. It runs single-threaded; each
//! privilege change is reverted before the next step.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const STAGING_PASSWORD: &str = "disposable_staging_pw";

fn admin_options() -> PgConnectOptions {
    let url = std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
        .expect("RUST_TEMPLATE_ADMIN_DATABASE_URL must name the disposable server");
    PgConnectOptions::from_str(&url).expect("parse the administrator URL")
}

async fn admin(statement: &str) {
    let mut connection = PgConnection::connect_with(&admin_options()).await.unwrap();
    // Fixed test statements, written in this file.
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

/// A deployed staging `.env` naming the disposable server.
fn environment_file(directory: &Path, extra: &[&str]) -> PathBuf {
    let options = admin_options();
    let (host, port) = (options.get_host().to_owned(), options.get_port());
    let mut lines = vec![
        "ENVIRONMENT=staging".to_owned(),
        format!("RELEASE_SHA={SHA}"),
        format!(
            "DATABASE_URL=postgresql+asyncpg://verdaxis_app_staging:{STAGING_PASSWORD}@{host}:{port}/verdaxis_staging"
        ),
    ];
    lines.extend(extra.iter().map(|line| (*line).to_owned()));
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(".env");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn preflight(environment_file: &Path) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args([
            "ops",
            "preflight",
            "--environment",
            "staging",
            "--release-sha",
            SHA,
        ])
        .arg("--environment-file")
        .arg(environment_file)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !stderr.contains(STAGING_PASSWORD),
        "no credential in output"
    );
    (output.status.code(), stderr)
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn preflight_attests_the_connected_role_and_capacity() {
    admin("DROP DATABASE IF EXISTS verdaxis_staging").await;
    admin("CREATE DATABASE verdaxis_staging").await;
    let directory =
        std::env::temp_dir().join(format!("verdaxis-p10-preflight-{}", std::process::id()));
    let deployed = environment_file(&directory, &[]);

    let (code, stderr) = preflight(&deployed);
    assert_eq!(code, Some(0), "{stderr}");

    // A role that can SET ROLE into another role is not least-privilege.
    admin("GRANT rust_group TO verdaxis_app_staging").await;
    let (code, stderr) = preflight(&deployed);
    admin("REVOKE rust_group FROM verdaxis_app_staging").await;
    assert_eq!(code, Some(3), "membership: {stderr}");
    assert!(stderr.contains("refused_role_privilege"), "{stderr}");

    admin("ALTER ROLE verdaxis_app_staging INHERIT").await;
    let (code, stderr) = preflight(&deployed);
    admin("ALTER ROLE verdaxis_app_staging NOINHERIT").await;
    assert_eq!(code, Some(3), "inherit: {stderr}");

    // A configured budget the server cannot hold: the Rust pool alone is the
    // server's whole max_connections.
    let mut connection = PgConnection::connect_with(&admin_options()).await.unwrap();
    let observed: String = sqlx::query_scalar("SHOW max_connections")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let slots = format!("READ_ADMISSION_SLOTS={observed}");
    let oversized = environment_file(&directory, &["DB_MAX_CONNECTIONS=1000", &slots]);
    let (code, stderr) = preflight(&oversized);
    assert_eq!(code, Some(3), "capacity: {stderr}");
    assert!(stderr.contains("refused_max_connections"), "{stderr}");

    let _ = std::fs::remove_dir_all(&directory);
    admin("DROP DATABASE verdaxis_staging").await;
}

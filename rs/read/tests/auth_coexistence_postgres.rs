//! Shared advisory-lock contention; HTTP coexistence is driven by the Python wrapper.
#![cfg(feature = "contract-tests")]
use sqlx::postgres::PgPoolOptions;
use verdaxis_read::auth::{lock::device_lock_key, token_bytes::token_digest};

#[cfg(test)]
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn python_and_rust_device_lock_contend_on_one_key() {
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("owner URL"))
        .await
        .unwrap();
    let digest = token_digest("synthetic-device-for-cross-stack-lock-check");
    let output = std::process::Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "venv/bin/python".into()))
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["-c", "import sys; from app.routers.auth_simple import _device_advisory_lock_key; print(_device_advisory_lock_key(sys.argv[1]))", &digest])
        .output().unwrap();
    assert!(output.status.success(), "Python key derivation failed");
    let python_key: i64 = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(device_lock_key(&digest).unwrap(), python_key);
    let mut first = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(python_key)
        .execute(&mut *first)
        .await
        .unwrap();
    let mut second = pool.acquire().await.unwrap();
    sqlx::query("SET lock_timeout = '200ms'")
        .execute(&mut *second)
        .await
        .unwrap();
    let refusal = sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(device_lock_key(&digest).unwrap())
        .execute(&mut *second)
        .await
        .unwrap_err();
    assert_eq!(
        refusal.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    first.commit().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(python_key)
        .execute(&mut *second)
        .await
        .unwrap();
}

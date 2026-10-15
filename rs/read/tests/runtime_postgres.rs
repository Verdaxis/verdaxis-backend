//! Requires pre-provisioned disposable roles; never targets deployed databases.
use verdaxis_read::runtime::health::probe;
use verdaxis_read::{Config, compiled_sha, connect, validate_config};

fn config(variable: &str) -> Config {
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var(variable).expect("explicit disposable test URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        pool_max: 9,
        python_budget: 52,
        configured_max_connections: 100,
    };
    validate_config(&config).expect("numeric-loopback _test database required");
    config
}

#[tokio::test]
async fn readiness_failure_has_no_identity() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/unreachable_test")
        .unwrap();
    let response = probe(&pool, "staging", &"a".repeat(40)).await;
    assert_eq!(response.0, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.1,
        serde_json::json!({"status":"error","db":"unavailable"})
    );
    pool.close().await;
}

#[tokio::test]
async fn raw_role_pool_and_health_contract() {
    let config = config("RUST_TEST_DATABASE_URL");
    let pool = connect(&config)
        .await
        .unwrap_or_else(|_| panic!("safe app role refused"));
    assert_eq!(pool.read.options().get_max_connections(), 8);
    assert_eq!(pool.health.options().get_max_connections(), 1);
    let health = pool.health.clone();
    let pool = pool.read;
    assert_eq!(
        probe(&health, "test", compiled_sha()).await.0,
        axum::http::StatusCode::OK
    );
    health.close().await;
    assert_eq!(pool.options().get_min_connections(), 0);
    let response = probe(&pool, "test", compiled_sha()).await;
    assert_eq!(response.0, axum::http::StatusCode::OK);
    assert_eq!(
        response.1,
        serde_json::json!({"status":"ok","db":"ok","environment":"test","release_sha":compiled_sha()})
    );
    for (setting, expected) in [
        ("statement_timeout", "30s"),
        ("lock_timeout", "3s"),
        ("idle_in_transaction_session_timeout", "1min"),
        ("default_transaction_read_only", "on"),
    ] {
        let actual: String = sqlx::query_scalar("SELECT current_setting($1)")
            .bind(setting)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, expected);
    }
    let error = sqlx::query("SELECT version_num FROM alembic_version")
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    let observed: String = sqlx::query_scalar("SHOW max_connections")
        .fetch_one(&pool)
        .await
        .unwrap();
    let observed: u32 = observed.parse().unwrap();
    let mut over_budget = config.clone();
    over_budget.python_budget = observed - 8;
    over_budget.configured_max_connections = observed + 1;
    assert!(connect(&over_budget).await.is_err());
    pool.close().await;
}

#[tokio::test]
async fn role_membership_refuses_startup() {
    // The fixture grants this LOGIN NOINHERIT role membership in another role.
    assert!(
        connect(&config("RUST_TEST_MEMBER_DATABASE_URL"))
            .await
            .is_err()
    );
}

#[test]
fn singleton_revision_uses_existing_migrator_verifier() {
    let app = config("RUST_TEST_DATABASE_URL");
    let migrator = config("RUST_TEST_MIGRATOR_DATABASE_URL");
    let python =
        std::env::var("RUST_TEST_PYTHON").expect("explicit project Python interpreter required");
    let result = std::process::Command::new(python)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/runtime_revision.py"
        ))
        .env_clear()
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("DATABASE_URL", app.database_url)
        .env("MIGRATOR_DATABASE_URL", migrator.database_url)
        .status()
        .expect("run existing revision verifier");
    assert!(result.success());
}

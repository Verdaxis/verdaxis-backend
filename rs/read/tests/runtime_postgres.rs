//! Requires pre-provisioned disposable roles; never targets deployed databases.
use verdaxis_read::runtime::health::probe;
use verdaxis_read::{Config, compiled_sha, connect, validate_config};

fn config(variable: &str) -> Config {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var(variable).expect("explicit disposable test URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
    };
    validate_config(&config).expect("numeric-loopback _test database required");
    config
}

#[tokio::test]
async fn explicit_timezone_applies_to_every_connection_and_checks_database_default() {
    let mut config = config("RUST_TEST_DATABASE_URL");
    let owner =
        sqlx::PgConnection::connect(&std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").unwrap())
            .await
            .unwrap();
    use sqlx::Connection;
    let mut owner = owner;
    for zone in ["Asia/Singapore", "UTC"] {
        sqlx::query(if zone == "UTC" {
            "ALTER DATABASE verdaxis_rust_test SET TimeZone TO 'UTC'"
        } else {
            "ALTER DATABASE verdaxis_rust_test SET TimeZone TO 'Asia/Singapore'"
        })
        .execute(&mut owner)
        .await
        .unwrap();
        config.db_session_time_zone = zone.into();
        let pools = connect(&config).await.unwrap();
        reference_boundaries(&mut owner, &config, &pools, zone).await;
        let mut held = Vec::new();
        for _ in 0..config.read_admission_slots {
            let mut connection = pools.read.acquire().await.unwrap();
            let actual: String = sqlx::query_scalar("SHOW TimeZone")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            assert_eq!(actual, zone);
            held.push(connection);
        }
        let health_zone: String = sqlx::query_scalar("SHOW TimeZone")
            .fetch_one(&pools.health)
            .await
            .unwrap();
        assert_eq!(health_zone, zone);
        // Force an admitted query onto the remaining connection. Its error
        // closes that connection; the replacement must run after_connect too.
        let mut old = held.pop().unwrap();
        let old_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *old)
            .await
            .unwrap();
        drop(old);
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let state = verdaxis_read::AppState::new(config.clone(), pools.clone(), policy).unwrap();
        let result: Result<(), verdaxis_read::ApiError> =
            verdaxis_read::run_admitted(&state, |connection| {
                Box::pin(async move {
                    sqlx::query("SELECT 1 / 0")
                        .execute(connection)
                        .await
                        .map_err(|_| verdaxis_read::ApiError::internal())?;
                    Ok(())
                })
            })
            .await;
        assert!(result.is_err());
        let mut replacement = pools.read.acquire().await.unwrap();
        let new_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *replacement)
            .await
            .unwrap();
        assert_ne!(old_pid, new_pid);
        let actual: String = sqlx::query_scalar("SHOW TimeZone")
            .fetch_one(&mut *replacement)
            .await
            .unwrap();
        assert_eq!(actual, zone);
        replacement.close().await.unwrap();
        drop(held);
        reference_boundaries(&mut owner, &config, &pools, zone).await;
        pools.read.close().await;
        pools.health.close().await;
        config.db_session_time_zone = if zone == "UTC" {
            "Asia/Singapore"
        } else {
            "UTC"
        }
        .into();
        assert!(connect(&config).await.is_err());
    }
    sqlx::query("ALTER DATABASE verdaxis_rust_test RESET TimeZone")
        .execute(&mut owner)
        .await
        .unwrap();
}

// The runtime fixture has no application schema. These immutable rows exercise
// the real reference SQL and HTTP serializers without bypassing deployed guards.
async fn reference_boundaries(
    owner: &mut sqlx::PgConnection,
    config: &Config,
    pools: &verdaxis_read::runtime::db::Pools,
    zone: &str,
) {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let policy: verdaxis_read::MarketPolicy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let product = &policy.products[0];
    let point = &policy.points[0];
    sqlx::query("CREATE TABLE IF NOT EXISTS trades (id uuid, product_id uuid, product_name text, fuel_type text, fuel_grade text, market_product text, delivery_point_id uuid, delivery_point_name text, delivery_point_region text, availability_window text, market_snapshot_version integer, status text, confirmed_at timestamptz, buyer_provenance text, seller_provenance text, quantity_mt numeric, price_per_mt_usd numeric)").execute(&mut *owner).await.unwrap();
    sqlx::query("GRANT SELECT ON trades TO rust_app")
        .execute(&mut *owner)
        .await
        .unwrap();
    sqlx::query("TRUNCATE trades")
        .execute(&mut *owner)
        .await
        .unwrap();
    for time in [
        "2026-10-15T15:59:59.999999Z",
        "2026-10-15T16:00:00Z",
        "2026-10-15T16:00:00.000001Z",
        "2026-10-15T23:59:59.999999Z",
        "2026-10-16T00:00:00Z",
        "2026-10-16T00:00:00.000001Z",
    ] {
        let time: chrono::DateTime<chrono::Utc> = time.parse().unwrap();
        sqlx::query("INSERT INTO trades VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'SPOT',1,'CONFIRMED',$10,'REAL','REAL',1.00,2.00)")
            .bind(uuid::Uuid::new_v4()).bind(product.id).bind(&product.name).bind(&product.fuel_type).bind(&product.fuel_grade).bind(&product.code).bind(point.id).bind(&point.name).bind(&point.region).bind(time).execute(&mut *owner).await.unwrap();
    }
    let state = verdaxis_read::AppState::new(config.clone(), pools.clone(), policy).unwrap();
    let app =
        verdaxis_read::runtime::http::router_with(verdaxis_read::routes::prices::routes(), state);
    for (date, count) in [
        ("2026-10-15", if zone == "UTC" { 4 } else { 1 }),
        ("2026-10-16", if zone == "UTC" { 2 } else { 5 }),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/prices/reference?date_from={date}&date_to={date}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(body["prices"].as_array().unwrap().len(), 1);
        let row = &body["prices"][0];
        assert_eq!(row["date"], date);
        assert_eq!(row["trade_count"], count);
        assert_eq!(row["vwap_usd"], "2.00");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/prices/reference/export?from_date={date}&to_date={date}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let csv = to_bytes(response.into_body(), 10000).await.unwrap();
        assert_eq!(
            csv.as_ref(),
            verdaxis_read::routes::prices::reference_csv(std::slice::from_ref(row)).unwrap()
        );
    }
    sqlx::query("DROP TABLE trades")
        .execute(owner)
        .await
        .unwrap();
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
    assert_eq!(
        pool.read.options().get_max_connections(),
        config.read_admission_slots
    );
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

#[tokio::test]
async fn caching_stays_off_against_a_schema_without_the_cache_checkpoint() {
    use verdaxis_read::runtime::cache_authority::database_state;

    // This database carries only the Alembic row of an earlier checkpoint. The
    // service must start, and the cache authority must report nothing to serve.
    let config = config("RUST_TEST_DATABASE_URL");
    let pools = connect(&config).await.unwrap();
    let mut connection = pools.read.acquire().await.unwrap();
    assert!(database_state(&mut connection).await.unwrap().is_none());
    drop(connection);
    pools.read.close().await;
    pools.health.close().await;
}

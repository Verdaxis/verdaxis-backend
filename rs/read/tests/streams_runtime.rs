use axum::{body::Body, http::Request};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tower::ServiceExt;
use verdaxis_read::runtime::{config::validate_config, db::Pools};
use verdaxis_read::streams::{
    bus::{PublicChannel, Terminal},
    policy::{MAX_CHANNEL_SUBSCRIBERS, MAX_PROCESS_SUBSCRIBERS, MAX_QUEUED_EVENTS},
    runtime::{CapacityConfig, RestartBudget, start_streams},
};
use verdaxis_read::{AppState, Config, compiled_sha};

fn config() -> Config {
    serde_json::from_value(serde_json::json!({
        "environment":"test", "release_sha":compiled_sha(),
        "migration_revision":"sse_20261021_replay_floor",
        "database_url":"postgresql://verdaxis_app_test:fixture@127.0.0.1:1/verdaxis_analytics_test",
        "db_session_time_zone":"UTC", "bind":"127.0.0.1:0",
        "read_admission_slots":2, "pool_max":3, "python_budget":52,
        "configured_max_connections":100,
        "phase2": {"jwt_secret":"fixture-stream-secret-at-least-thirty-two-characters",
            "jwt_issuer":"fixture", "jwt_audience":"fixture", "frontend_url":"https://example.test",
            "email_from":"fixture@example.test", "resend_api_key":"fixture-mail-key"}
    }))
    .unwrap()
}

fn budget(config: &Config, cache: u32, streams: u32) -> String {
    serde_json::json!({"version":1,"release_sha":config.release_sha,
        "environment_digest":"a".repeat(64), "services":2,"workers":4,
        "pool":2,"overflow":1,"reserve":20,"rust_pool":config.pool_max,
        "python_budget":52,"cache_listener":cache,"stream_listener":streams,
        "total":52+config.pool_max+cache+streams,"configured_max_connections":config.configured_max_connections
    }).to_string()
}

fn state(mut config: Config) -> AppState {
    // Offline tests never open a socket, even when the runtime tries admission.
    config.phase2 = Default::default();
    let read = PgPoolOptions::new()
        .max_connections(2)
        .connect_lazy(
            "postgresql://unused:unused@localhost/unused?host=/nonexistent-stream-runtime",
        )
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(
            "postgresql://unused:unused@localhost/unused?host=/nonexistent-stream-runtime",
        )
        .unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    AppState::new(config, Pools { read, health }, policy).unwrap()
}

#[test]
fn nonproduction_new_revision_allowed() {
    assert!(validate_config(&config()).is_ok());
    let mut production = config();
    production.environment = "production".into();
    assert!(validate_config(&production).is_err());
}

#[test]
fn unlisted_revision_is_refused() {
    let mut config = config();
    config.migration_revision = "unlisted_stream_revision".into();
    assert!(validate_config(&config).is_err());
}

#[test]
fn stream_capacity_config_is_bounded() {
    assert!(CapacityConfig::default().policy().is_ok());
    for invalid in [
        CapacityConfig {
            per_channel: 0,
            ..Default::default()
        },
        CapacityConfig {
            per_channel: MAX_CHANNEL_SUBSCRIBERS + 1,
            ..Default::default()
        },
        CapacityConfig {
            process: MAX_PROCESS_SUBSCRIBERS + 1,
            ..Default::default()
        },
        CapacityConfig {
            queue: MAX_QUEUED_EVENTS + 1,
            ..Default::default()
        },
        CapacityConfig {
            page_rows: 501,
            ..Default::default()
        },
        CapacityConfig {
            page_bytes: 2 * 1024 * 1024 + 1,
            ..Default::default()
        },
        CapacityConfig {
            event_bytes: 256 * 1024 + 1,
            ..Default::default()
        },
    ] {
        assert!(invalid.policy().is_err());
    }
}

#[test]
fn enabled_listeners_keep_the_full_python_budget() {
    use verdaxis_read::runtime::config::{runtime_connection_total, validate_prepared_budget};
    let mut config = config();
    config.prepared_budget = Some(budget(&config, 1, 1));
    assert_eq!(runtime_connection_total(&config).unwrap(), 57);
    assert!(validate_prepared_budget(&config, config.prepared_budget.as_ref().unwrap()).is_ok());
    assert!(verdaxis_read::validate_budget(57, 56, 100).is_err());
    assert!(verdaxis_read::validate_budget(57, 100, 56).is_err());
    let mut artifact: serde_json::Value =
        serde_json::from_str(config.prepared_budget.as_ref().unwrap()).unwrap();
    artifact["workers"] = 1.into();
    assert!(validate_prepared_budget(&config, &artifact.to_string()).is_err());
    artifact["release_sha"] = "b".repeat(40).into();
    assert!(validate_prepared_budget(&config, &artifact.to_string()).is_err());
}

#[tokio::test]
async fn stream_startup_failure_keeps_rest_available() {
    let mut config = config();
    config.prepared_budget = Some(budget(&config, 0, 1));
    let mut state = state(config);
    assert!(
        start_streams(&mut state, CapacityConfig::default())
            .await
            .is_none()
    );
    let response = verdaxis_read::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn stream_failure_readiness_is_not_ok() {
    let mut config = config();
    config.prepared_budget = Some(budget(&config, 0, 1));
    let state = state(config);
    assert!(!verdaxis_read::streams::runtime::ready(&state));
}

#[test]
fn subsystem_restart_budget_is_finite() {
    let mut budget = RestartBudget::default();
    for _ in 0..3 {
        assert_eq!(budget.next_delay(), Some(Duration::from_secs(30)));
    }
    assert_eq!(budget.next_delay(), None);
    assert_eq!(budget.next_delay(), None);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL stream runtime fixture"]
async fn stream_runtime_database_lifecycle() {
    let mut config = config();
    config.database_url = std::env::var("STREAM_RUNTIME_APP_URL").unwrap();
    config.prepared_budget = Some(budget(&config, 0, 1));
    let mut observed_limit = config.clone();
    observed_limit.python_budget = 97;
    observed_limit.configured_max_connections = 200;
    observed_limit.prepared_budget = Some(budget(&observed_limit, 0, 1));
    assert!(verdaxis_read::connect(&observed_limit).await.is_err());
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let mut state = AppState::new(config, pools.clone(), policy).unwrap();
    let admin = sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_ADMIN_URL").unwrap()).await.unwrap();
    let floor: i64 = sqlx::query_scalar("SELECT discarded_through_seq FROM market_event_replay_state").fetch_one(&admin).await.unwrap();
    sqlx::query("DELETE FROM market_event_replay_state").execute(&admin).await.unwrap();
    assert!(start_streams(&mut state, CapacityConfig::default()).await.is_none());
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    sqlx::query("INSERT INTO market_event_replay_state(singleton,discarded_through_seq) VALUES (true,$1)").bind(floor).execute(&admin).await.unwrap();
    admin.close().await;
    let runtime = start_streams(&mut state, CapacityConfig::default())
        .await
        .unwrap();
    assert!(verdaxis_read::streams::runtime::ready(&state));
    let hub = state.streams.as_ref().unwrap().hub.clone();
    let mut subscription = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let owner = sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_OWNER_URL").unwrap())
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name='verdaxis-stream-election'",
    )
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(count, 1);
    // Losing the physical listener is recoverable within the actor's budget.
    let pid: i32 = sqlx::query_scalar(
        "SELECT pid FROM pg_stat_activity WHERE application_name='verdaxis-stream-election'",
    )
    .fetch_one(&owner)
    .await
    .unwrap();
    let admin = sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_ADMIN_URL").unwrap())
        .await
        .unwrap();
    sqlx::query("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let replacement: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE application_name='verdaxis-stream-election' AND pid <> $1")
                .bind(pid).fetch_optional(&owner).await.unwrap();
            if replacement.is_some() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await.unwrap();
    runtime.shutdown().await;
    assert_eq!(subscription.wait_terminal().await, Terminal::Shutdown);
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    assert!(hub.subscribe_public(PublicChannel::Prices).is_err());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name='verdaxis-stream-election'",
    )
    .fetch_one(&owner)
    .await
    .unwrap();
    assert_eq!(count, 0);
    let response = verdaxis_read::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        !axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    owner.close().await;
    pools.read.close().await;
    pools.health.close().await;
}

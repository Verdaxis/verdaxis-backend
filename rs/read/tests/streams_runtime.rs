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

fn state(config: Config) -> AppState {
    // Offline tests never open a socket, even when the runtime tries admission.
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
    let runtime = start_streams(&mut state, CapacityConfig::default())
        .await
        .unwrap();
    assert!(!verdaxis_read::streams::runtime::ready(&state));
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
    runtime.shutdown().await;
}

#[tokio::test]
async fn stream_failure_readiness_is_not_ok() {
    let mut config = config();
    config.prepared_budget = Some(budget(&config, 0, 1));
    let state = state(config);
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    let response = verdaxis_read::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body,
        serde_json::json!({"status":"error","db":"unavailable","environment":"test","release_sha":compiled_sha()})
    );
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
    observed_limit.read_admission_slots = 47;
    observed_limit.pool_max = 48;
    observed_limit.configured_max_connections = 200;
    observed_limit.prepared_budget = Some(budget(&observed_limit, 0, 1));
    assert!(verdaxis_read::connect(&observed_limit).await.is_err());
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let mut state = AppState::new(config, pools.clone(), policy).unwrap();
    let admin = sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_ADMIN_URL").unwrap())
        .await
        .unwrap();
    let floor: i64 =
        sqlx::query_scalar("SELECT discarded_through_seq FROM market_event_replay_state")
            .fetch_one(&admin)
            .await
            .unwrap();
    sqlx::query("DELETE FROM market_event_replay_state")
        .execute(&admin)
        .await
        .unwrap();
    let runtime = start_streams(&mut state, CapacityConfig::default())
        .await
        .unwrap();
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    sqlx::query(
        "INSERT INTO market_event_replay_state(singleton,discarded_through_seq) VALUES (true,$1)",
    )
    .bind(floor)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    tokio::time::timeout(Duration::from_secs(40), async {
        while !verdaxis_read::streams::runtime::ready(&state) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
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
    // Removing the durable boundary stops dispatch and closes existing bodies.
    let admin = sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_ADMIN_URL").unwrap())
        .await
        .unwrap();
    sqlx::query("DELETE FROM market_event_replay_state")
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), subscription.wait_terminal())
            .await
            .unwrap(),
        Terminal::Unavailable
    );
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    sqlx::query(
        "INSERT INTO market_event_replay_state(singleton,discarded_through_seq) VALUES (true,$1)",
    )
    .bind(floor)
    .execute(&admin)
    .await
    .unwrap();
    runtime.shutdown().await;
    admin.close().await;
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

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL stream runtime fixture"]
async fn invalid_persisted_row_resets_then_dispatches_without_restart() {
    use verdaxis_read::streams::{
        dispatch::{Dispatch, PgDispatchStore},
        store,
    };
    let mut config = config();
    config.database_url = std::env::var("STREAM_RUNTIME_APP_URL").unwrap();
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let admin = sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_ADMIN_URL").unwrap())
        .await
        .unwrap();
    let hub = verdaxis_read::streams::bus::StreamHub::default();
    let mut source = PgDispatchStore::new(pools.read.clone());
    let mut dispatch = Dispatch::initialize(&mut source, hub.clone())
        .await
        .unwrap();
    let org = uuid::Uuid::new_v4();
    let mut private = hub.subscribe_org(org).unwrap();
    let mut public = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let mut sequences = Vec::new();
    for payload in [
        serde_json::json!({"oversized":"x".repeat(256*1024)}),
        serde_json::json!({"trade_id":"fixture","_public_market_invalidation":true}),
    ] {
        let seq: i64 = sqlx::query_scalar("SELECT nextval('market_event_stream_seq')")
            .fetch_one(&admin)
            .await
            .unwrap();
        sequences.push(seq);
        sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,stream_seq) VALUES ($1,'trade_created','trade','fixture',$2,$3,$4)")
            .bind(uuid::Uuid::new_v4()).bind(serde_json::json!([org])).bind(payload).bind(seq).execute(&admin).await.unwrap();
    }
    dispatch.turn(&mut source).await.unwrap();
    assert_eq!(private.wait_terminal().await, Terminal::InvalidEvent);
    assert_eq!(public.wait_terminal().await, Terminal::InvalidEvent);
    assert_eq!(dispatch.cursor(), *sequences.last().unwrap());
    assert!(hub.is_available());
    assert!(
        store::replay_page(
            &pools.read,
            org,
            sequences[0] - 1,
            sequences[1],
            Default::default()
        )
        .await
        .is_err()
    );
    let mut fresh = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let seq: i64 = sqlx::query_scalar("SELECT nextval('market_event_stream_seq')")
        .fetch_one(&admin)
        .await
        .unwrap();
    sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,stream_seq) VALUES ($1,'trade_created','trade','fixture',$2,$3,$4)")
        .bind(uuid::Uuid::new_v4()).bind(serde_json::json!([org])).bind(serde_json::json!({"_public_market_invalidation":true})).bind(seq).execute(&admin).await.unwrap();
    dispatch.turn(&mut source).await.unwrap();
    assert!(matches!(
        fresh.recv().await,
        Some(verdaxis_read::streams::bus::Delivery::PublicInvalidation)
    ));
    pools.read.close().await;
    pools.health.close().await;
    admin.close().await;
}

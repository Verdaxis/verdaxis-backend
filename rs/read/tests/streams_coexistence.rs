use std::time::Duration;
use verdaxis_read::{Config, compiled_sha};
use verdaxis_read::streams::{bus::{PublicChannel, Terminal}, runtime::{CapacityConfig, start_streams}};

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL stream fixture"]
async fn three_supervisor_cycles_preserve_nonzero_baseline_and_release_sessions() {
    let mut config: Config = serde_json::from_value(serde_json::json!({
        "environment":"test","release_sha":compiled_sha(),"migration_revision":"sse_20261021_replay_floor",
        "database_url":std::env::var("STREAM_RUNTIME_APP_URL").unwrap(),"db_session_time_zone":"UTC",
        "bind":"127.0.0.1:0","read_admission_slots":2,"pool_max":3,"python_budget":52,"configured_max_connections":100,
        "phase2":{"jwt_secret":"fixture-stream-secret-at-least-thirty-two-characters","jwt_issuer":"fixture","jwt_audience":"fixture",
            "frontend_url":"https://example.test","email_from":"fixture@example.test","resend_api_key":"fixture"}
    })).unwrap();
    config.prepared_budget=Some(serde_json::json!({"version":1,"release_sha":compiled_sha(),"environment_digest":"a".repeat(64),
        "services":2,"workers":4,"pool":2,"overflow":1,"reserve":20,"rust_pool":3,"python_budget":52,
        "stream_listener":1,"total":56,"configured_max_connections":100}).to_string());
    let admin=sqlx::PgPool::connect(&std::env::var("STREAM_RUNTIME_ADMIN_URL").unwrap()).await.unwrap();
    let baseline:i64=sqlx::query_scalar("SELECT nextval('market_event_stream_seq')").fetch_one(&admin).await.unwrap();
    let event=uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,stream_seq) VALUES ($1,'trade_created','trade',$2,'[]','{}',$3)")
        .bind(event).bind(event.to_string()).bind(baseline).execute(&admin).await.unwrap();
    let pools=verdaxis_read::connect(&config).await.unwrap();
    let policy=serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let mut state=verdaxis_read::AppState::new(config,pools.clone(),policy).unwrap();
    let runtime=start_streams(&mut state,CapacityConfig::default()).await.unwrap();
    let hub=state.streams.as_ref().unwrap().hub.clone();
    assert!(verdaxis_read::streams::runtime::ready(&state));
    for cycle in 1..=3 {
        let mut subscription=hub.subscribe_public(PublicChannel::Prices).unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(50),subscription.recv()).await.is_err());
        let floor:i64=sqlx::query_scalar("SELECT discarded_through_seq FROM market_event_replay_state").fetch_one(&admin).await.unwrap();
        sqlx::query("DELETE FROM market_event_replay_state").execute(&admin).await.unwrap();
        assert_eq!(tokio::time::timeout(Duration::from_secs(5),subscription.wait_terminal()).await.unwrap(),Terminal::Unavailable);
        assert!(!verdaxis_read::streams::runtime::ready(&state));
        sqlx::query("INSERT INTO market_event_replay_state(singleton,discarded_through_seq) VALUES (true,$1)").bind(floor).execute(&admin).await.unwrap();
        tokio::time::timeout(Duration::from_secs(40),async {
            while !verdaxis_read::streams::runtime::ready(&state) {tokio::time::sleep(Duration::from_millis(25)).await;}
        }).await.unwrap();
        let listeners:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='verdaxis-stream-election'").fetch_one(&admin).await.unwrap();
        assert_eq!(listeners,1);
        println!("cycle={cycle} baseline={baseline} terminal=stream_unavailable listeners={listeners}");
    }
    let mut subscription=hub.subscribe_public(PublicChannel::Prices).unwrap();
    sqlx::query("DELETE FROM market_event_replay_state").execute(&admin).await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5),subscription.wait_terminal()).await.unwrap(),Terminal::Unavailable);
    runtime.shutdown().await;
    sqlx::query("INSERT INTO market_event_replay_state(singleton,discarded_through_seq) VALUES (true,0)").execute(&admin).await.unwrap();
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    let listeners:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='verdaxis-stream-election'").fetch_one(&admin).await.unwrap();
    assert_eq!(listeners,0);
    pools.read.close().await;pools.health.close().await;admin.close().await;
}

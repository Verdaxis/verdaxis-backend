//! Disposable line-oriented stream actor controls; requires contract-tests.
use std::{io::{self, Write}, sync::Arc};
use futures_util::future::BoxFuture;
use sqlx::PgConnection;
use verdaxis_read::{Config, compiled_sha};
use verdaxis_read::streams::election::{ElectionSession, SessionSettings, Stage, StageHook};

fn emit(value: serde_json::Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

struct Barrier;
impl StageHook for Barrier {
    fn at<'a>(&'a self, stage: Stage, pid: u32, _connection: &'a mut PgConnection) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == Stage::Assigned {
                emit(serde_json::json!({"stage":"assigned", "pid":pid}));
                let mut line = String::new();
                io::stdin().read_line(&mut line).unwrap();
                assert_eq!(line.trim(),"continue");
            }
            Ok(())
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let config: Config = serde_json::from_value(serde_json::json!({
        "environment":"test", "release_sha":compiled_sha(),
        "migration_revision":"sse_20261021_replay_floor",
        "database_url":std::env::var("STREAM_FIXTURE_APP_URL").unwrap(),
        "db_session_time_zone":"UTC", "bind":"127.0.0.1:0",
        "read_admission_slots":2,"pool_max":3,"python_budget":52,"configured_max_connections":100
    })).unwrap();
    verdaxis_read::validate_config(&config).unwrap();
    let mut settings = SessionSettings::from_config(&config).unwrap();
    if std::env::var("STREAM_FIXTURE_BARRIER").as_deref() == Ok("assigned") {
        settings = settings.with_stage_hook(Arc::new(Barrier));
    }
    let mut session = ElectionSession::open(&settings).await.unwrap();
    emit(serde_json::json!({"stage":"ready","pid":session.backend_pid()}));
    loop {
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap() == 0 { break; }
        match line.trim() {
            "assign" => {
                let result = session.assign_one_batch().await;
                emit(serde_json::json!({"stage":"result","outcome":format!("{result:?}")}));
            }
            "stop" => break,
            _ => panic!("unknown fixture operation"),
        }
    }
    session.close().await;
    emit(serde_json::json!({"stage":"closed"}));
}

//! Cancellation and capacity ownership against disposable PostgreSQL.
//!
//! Requires a disposable database and roles; never targets deployed databases.
use std::{sync::Arc, time::Duration};
use verdaxis_read::{AppState, Config, MarketPolicy, compiled_sha, connect, run_admitted};

const SLOW: &str = "SELECT pg_sleep(5)";

#[tokio::test]
async fn admitted_unexpected_and_database_failures_keep_distinct_types() {
    if std::env::var("RUST_TEST_DATABASE_URL").is_err() {
        return;
    }
    let (state, observer, pool) = state().await;
    let error = run_admitted::<()>(&state, |_| {
        Box::pin(async { Err(verdaxis_read::ApiError::internal()) })
    })
    .await
    .unwrap_err();
    assert_eq!(error.kind, verdaxis_read::ApiErrorKind::Unexpected);
    assert_eq!(state.reads.available_permits(), 8);
    let error = run_admitted::<()>(&state, |connection| {
        Box::pin(async move {
            sqlx::query("SELECT 1 / 0")
                .execute(connection)
                .await
                .map_err(|_| verdaxis_read::ApiError::database())?;
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert_eq!(error.kind, verdaxis_read::ApiErrorKind::Database);
    assert_eq!(state.reads.available_permits(), 8);
    let value = run_admitted(&state, |connection| {
        Box::pin(async move {
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(connection)
                .await
                .map_err(|_| verdaxis_read::ApiError::database())
        })
    })
    .await
    .unwrap();
    assert_eq!(value, 1);
    pool.close().await;
    observer.close().await;
}

/// Build the admitted state and keep the readiness pool as an out-of-band
/// observer that never consumes an admission slot.
async fn state() -> (Arc<AppState>, sqlx::PgPool, sqlx::PgPool) {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var("RUST_TEST_DATABASE_URL").expect("disposable URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        pool_max: 9,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
    };
    verdaxis_read::validate_config(&config).expect("valid disposable configuration");
    let pools = connect(&config).await.expect("connect");
    let observer = pools.health.clone();
    let read = pools.read.clone();
    let state = AppState::new(
        config,
        pools,
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .expect("state");
    (state.into(), observer, read)
}

/// App-role connections used by the process: `read` capacity plus readiness.
async fn connections(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE usename = current_user")
        .fetch_one(pool)
        .await
        .expect("count app-role connections")
}

async fn active_reads(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE usename = current_user \
         AND query = $1 AND state = 'active'",
    )
    .bind(SLOW)
    .fetch_one(pool)
    .await
    .expect("count active reads")
}

async fn wait_for<F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>>(
    what: &str,
    reached: F,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !reached().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn cancelled_query_keeps_slot_until_drop() {
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(std::io::stderr)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let (state, observer, read) = state().await;
    let app = verdaxis_read::runtime::http::middleware_with_state(
        axum::Router::new().route(
            "/api/ports",
            axum::routing::get({
                let state = state.clone();
                move || {
                    let state = state.clone();
                    async move {
                        run_admitted(&state, |connection| {
                            Box::pin(async move {
                                sqlx::query(SLOW)
                                    .execute(connection)
                                    .await
                                    .map_err(|error| {
                                        verdaxis_read::runtime::admission::log_sql_error(
                                            "work", &error,
                                        );
                                        verdaxis_read::ApiError::internal()
                                    })?;
                                Ok(())
                            })
                        })
                        .await
                        .map(|_| axum::Json(serde_json::json!({"ok": true})))
                    }
                }
            }),
        ),
        (*state).clone(),
    );
    use tower::ServiceExt;

    // Cycle 0 cancels eight clients while their queries are in flight; cycle 1
    // then proves the slots and pool connections are reusable.
    for cycle in 0..2 {
        let mut callers = Vec::new();
        for _ in 0..8 {
            let app = app.clone();
            callers.push(tokio::spawn(async move {
                app.oneshot(
                    axum::http::Request::builder()
                        .uri("/api/ports")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }));
        }
        {
            let observer = observer.clone();
            wait_for("all eight reads to start", move || {
                let observer = observer.clone();
                Box::pin(async move { active_reads(&observer).await == 8 })
            })
            .await;
        }
        // The ninth concurrent request is refused at the gate with the exact
        // busy contract, without touching the exhausted pool.
        let rejected = run_admitted(&state, |_| Box::pin(async { Ok(()) }))
            .await
            .expect_err("ninth request refused");
        assert_eq!(rejected.status, 503);
        assert_eq!(rejected.retry_after, Some(1));
        assert_eq!(state.reads.available_permits(), 0);
        assert!(
            connections(&observer).await <= 9,
            "app role must never exceed the nine-connection budget"
        );

        if cycle == 0 {
            for caller in callers {
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
            }
            // Cancellation detaches the caller; it must not cancel the query or
            // release admission while the database is still busy.
            assert!(
                active_reads(&observer).await > 0,
                "a client cancellation cancelled database work"
            );
            assert_eq!(state.reads.available_permits(), 0);
        } else {
            for caller in callers {
                let response = tokio::time::timeout(Duration::from_secs(20), caller)
                    .await
                    .expect("work must not wait on the pool acquisition timeout")
                    .unwrap();
                assert_eq!(response.status(), 200);
            }
        }

        {
            let state = state.clone();
            let observer = observer.clone();
            wait_for("admission to reopen", move || {
                let state = state.clone();
                let observer = observer.clone();
                Box::pin(async move {
                    assert!(connections(&observer).await <= 9);
                    state.reads.available_permits() == 8
                })
            })
            .await;
        }
    }

    read.close().await;
    observer.close().await;
}

#[derive(Clone)]
struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn cancelled_error_closes_before_replacement_and_logs_stage() {
    let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let (state, observer, read) = state().await;
    let release = Arc::new(tokio::sync::Notify::new());
    let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(8);
    let mut callers = Vec::new();
    for _ in 0..8 {
        let state = state.clone();
        let release = release.clone();
        let started = started_tx.clone();
        callers.push(tokio::spawn(async move {
            run_admitted(&state, move |connection| {
                Box::pin(async move {
                    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(&mut *connection)
                        .await
                        .unwrap();
                    started.send(pid).await.unwrap();
                    release.notified().await;
                    sqlx::query("SELECT 1/0 /* secret-sentinel */")
                        .execute(connection)
                        .await
                        .map_err(|error| {
                            verdaxis_read::runtime::admission::log_sql_error("work", &error);
                            verdaxis_read::database_error("/api/ports", "GET", Some("22012"))
                        })?;
                    Ok(())
                })
            })
            .await
        }));
    }
    let mut old_pids = Vec::new();
    for _ in 0..8 {
        old_pids.push(started_rx.recv().await.unwrap());
    }
    for caller in callers {
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
    }
    assert_eq!(state.reads.available_permits(), 0);
    let busy = run_admitted(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_eq!(busy.status, 503);
    assert_eq!(busy.retry_after, Some(1));
    release.notify_waiters();
    let ready = state.clone();
    wait_for("failed owners to close", move || {
        let ready = ready.clone();
        Box::pin(async move { ready.reads.available_permits() == 8 })
    })
    .await;
    let pid: i32 = run_admitted(&state, |connection| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(connection)
                .await
                .map_err(|_| verdaxis_read::ApiError::internal())
        })
    })
    .await
    .expect("fresh request after replacement");
    assert!(!old_pids.contains(&pid));
    assert_eq!(state.reads.available_permits(), 8);
    assert!(connections(&observer).await <= 9);
    let logs = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains("secret-sentinel"));
    let failures: Vec<serde_json::Value> = logs
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|v: &serde_json::Value| v["fields"]["message"] == "admitted_database_failure")
        .collect();
    assert!(!failures.is_empty(), "missing stage diagnostics: {logs}");
    assert_eq!(failures[0]["fields"]["stage"], "work");
    assert_eq!(failures[0]["fields"]["error_class"], "database");
    assert_eq!(failures[0]["fields"]["sqlstate"], "22012");
    read.close().await;
    observer.close().await;
}

#[tokio::test]
async fn news_and_market_reads_share_capacity_in_both_directions() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    for (occupier, rejected) in [
        ("/api/ports", "/api/news"),
        ("/api/news", "/api/catalog/products"),
    ] {
        let (state, observer, read) = state().await;
        let release = Arc::new(tokio::sync::Notify::new());
        let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(8);
        let held_state = state.clone();
        let held_release = release.clone();
        let held = axum::Router::new().route(
            occupier,
            axum::routing::get(move || {
                let state = held_state.clone();
                let release = held_release.clone();
                let started = started_tx.clone();
                async move {
                    run_admitted(&state, move |connection| {
                        Box::pin(async move {
                            sqlx::query("SELECT 1")
                                .execute(connection)
                                .await
                                .map_err(|error| {
                                    verdaxis_read::runtime::admission::log_sql_error(
                                        "work", &error,
                                    );
                                    verdaxis_read::ApiError::internal()
                                })?;
                            started.send(()).await.unwrap();
                            release.notified().await;
                            Ok(())
                        })
                    })
                    .await
                    .map(|_| "ok")
                }
            }),
        );
        let routes = if rejected == "/api/news" {
            held.merge(verdaxis_read::routes::news::routes())
        } else {
            held.merge(verdaxis_read::routes::catalog::routes())
        };
        let app = verdaxis_read::runtime::http::middleware_with_state(
            routes.with_state((*state).clone()),
            (*state).clone(),
        );
        let mut callers = Vec::new();
        for _ in 0..8 {
            let app = app.clone();
            callers.push(tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .uri(occupier)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }));
        }
        for _ in 0..8 {
            started_rx.recv().await.unwrap();
        }
        assert_eq!(state.reads.available_permits(), 0);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(rejected)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["retry-after"], "1");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"detail":"Public market data is temporarily busy; retry shortly."})
        );
        assert!(connections(&observer).await <= 9);
        release.notify_waiters();
        for caller in callers {
            assert_eq!(caller.await.unwrap().status(), 200);
        }
        assert_eq!(state.reads.available_permits(), 8);
        read.close().await;
        observer.close().await;
    }
}

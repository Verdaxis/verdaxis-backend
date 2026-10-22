//! Cancellation and capacity ownership against disposable PostgreSQL.
//!
//! Requires a disposable database and roles; never targets deployed databases.
use std::{sync::Arc, time::Duration};
use verdaxis_read::routes::market_data_error::database_failure;
use verdaxis_read::{
    ApiErrorKind, AppState, Config, MarketPolicy, compiled_sha, connect, run_admitted,
    run_market_read,
};

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
    assert_eq!(
        state.reads.available_permits(),
        state.config.read_admission_slots as usize
    );
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
    assert_eq!(
        state.reads.available_permits(),
        state.config.read_admission_slots as usize
    );
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
    state_with_slots(16).await
}

/// The same state with an exact number of admission slots and pooled
/// connections.
async fn state_with_slots(slots: u32) -> (Arc<AppState>, sqlx::PgPool, sqlx::PgPool) {
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var("RUST_TEST_DATABASE_URL").expect("disposable URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: slots,
        pool_max: slots + 1,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
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

    // Cycle 0 cancels configured clients while their queries are in flight; cycle 1
    // then proves the slots and pool connections are reusable.
    for cycle in 0..2 {
        let mut callers = Vec::new();
        for _ in 0..state.config.read_admission_slots {
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
            wait_for("all configured reads to start", move || {
                let observer = observer.clone();
                Box::pin(async move { active_reads(&observer).await == 16 })
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
            connections(&observer).await <= 17,
            "app role must never exceed the configured-connection budget"
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
                    assert!(connections(&observer).await <= 17);
                    state.reads.available_permits() == 16
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
    let (started_tx, mut started_rx) =
        tokio::sync::mpsc::channel(state.config.read_admission_slots as usize);
    let mut callers = Vec::new();
    for _ in 0..state.config.read_admission_slots {
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
    for _ in 0..state.config.read_admission_slots {
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
        Box::pin(async move { ready.reads.available_permits() == 16 })
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
    assert_eq!(
        state.reads.available_permits(),
        state.config.read_admission_slots as usize
    );
    assert!(connections(&observer).await <= 17);
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
        let (started_tx, mut started_rx) =
            tokio::sync::mpsc::channel(state.config.read_admission_slots as usize);
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
        for _ in 0..state.config.read_admission_slots {
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
        for _ in 0..state.config.read_admission_slots {
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
        assert!(connections(&observer).await <= 17);
        release.notify_waiters();
        for caller in callers {
            assert_eq!(caller.await.unwrap().status(), 200);
        }
        assert_eq!(
            state.reads.available_permits(),
            state.config.read_admission_slots as usize
        );
        read.close().await;
        observer.close().await;
    }
}

/// Check one typed market-data failure: status, code, retry hint and class.
fn assert_market_read_error(
    error: &verdaxis_read::ApiError,
    status: u16,
    code: &str,
    kind: ApiErrorKind,
) {
    assert_eq!(error.status, status);
    assert_eq!(error.detail["code"], code);
    assert_eq!(error.retry_after, (status == 503).then_some(1));
    assert_eq!(error.kind, kind);
}

/// The backend process of the next market read.
async fn market_read_backend(state: &AppState) -> i32 {
    run_market_read(state, |connection| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(connection)
                .await
                .map_err(|error| database_failure("probe", &error))
        })
    })
    .await
    .expect("market read")
}

#[tokio::test]
async fn market_read_uses_one_read_only_transaction_with_a_statement_limit() {
    let (state, observer, read) = state_with_slots(1).await;
    let (backend, read_only, statement_timeout, isolation): (i32, String, String, String) =
        run_market_read(&state, |connection| {
            Box::pin(async move {
                sqlx::query_as(
                    "SELECT pg_backend_pid(), current_setting('transaction_read_only'), \
                     current_setting('statement_timeout'), current_setting('transaction_isolation')",
                )
                .fetch_one(connection)
                .await
                .map_err(|error| database_failure("probe", &error))
            })
        })
        .await
        .unwrap();
    assert_eq!(read_only, "on");
    assert_eq!(statement_timeout, "5s");
    assert_eq!(isolation, "read committed");

    // The limit belongs to the transaction: the one pooled connection is back
    // with its session default for the next admitted read.
    let (next_backend, next_timeout): (i32, String) = run_admitted(&state, |connection| {
        Box::pin(async move {
            sqlx::query_as("SELECT pg_backend_pid(), current_setting('statement_timeout')")
                .fetch_one(connection)
                .await
                .map_err(|_| verdaxis_read::ApiError::database())
        })
    })
    .await
    .unwrap();
    assert_eq!(next_backend, backend);
    assert_eq!(next_timeout, "30s");

    // Domain SQL past the limit stops with the typed transient error.
    let started = std::time::Instant::now();
    let error = run_market_read::<()>(&state, |connection| {
        Box::pin(async move {
            sqlx::query("SELECT pg_sleep(10)")
                .execute(connection)
                .await
                .map_err(|error| database_failure("probe", &error))?;
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_market_read_error(&error, 503, "READ_UNAVAILABLE", ApiErrorKind::Database);
    assert_eq!(state.reads.available_permits(), 1);
    read.close().await;
    observer.close().await;
}

#[tokio::test]
async fn market_read_failures_are_typed_and_release_the_owner() {
    let (state, observer, read) = state_with_slots(1).await;
    let first = market_read_backend(&state).await;

    // Failed work closes its connection; the next read gets a new one.
    let error = run_market_read::<()>(&state, |connection| {
        Box::pin(async move {
            sqlx::query("SELECT 1 / 0")
                .execute(connection)
                .await
                .map_err(|error| database_failure("probe", &error))?;
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert_market_read_error(&error, 500, "READ_FAILED", ApiErrorKind::Database);
    assert_eq!(state.reads.available_permits(), 1);
    let second = market_read_backend(&state).await;
    assert_ne!(second, first);

    // A connection lost after the work cannot commit; it is not reused.
    let terminator = observer.clone();
    let error = run_market_read::<()>(&state, move |connection| {
        Box::pin(async move {
            let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *connection)
                .await
                .map_err(|error| database_failure("probe", &error))?;
            let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
                .bind(backend)
                .fetch_one(&terminator)
                .await
                .unwrap();
            assert!(terminated);
            let observer = terminator.clone();
            wait_for("the backend to end", move || {
                let observer = observer.clone();
                Box::pin(async move {
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM pg_stat_activity WHERE pid = $1",
                    )
                    .bind(backend)
                    .fetch_one(&observer)
                    .await
                    .unwrap()
                        == 0
                })
            })
            .await;
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert_market_read_error(&error, 500, "READ_FAILED", ApiErrorKind::Database);
    assert_eq!(state.reads.available_permits(), 1);
    let third = market_read_backend(&state).await;
    assert_ne!(third, second);

    // A panic ends the owner task without a result. The existing helper keeps
    // its string detail for the same failure.
    let error = run_market_read::<()>(&state, |_| {
        Box::pin(async {
            panic!("market read work stopped");
            #[allow(unreachable_code)]
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert_market_read_error(&error, 500, "READ_FAILED", ApiErrorKind::Unexpected);
    let existing = run_admitted::<()>(&state, |_| {
        Box::pin(async {
            panic!("admitted work stopped");
            #[allow(unreachable_code)]
            Ok(())
        })
    })
    .await
    .unwrap_err();
    assert_eq!(existing.status, 500);
    assert_eq!(existing.detail, "Database operation failed.");
    let ready = state.clone();
    wait_for("the stopped owners to release admission", move || {
        let ready = ready.clone();
        Box::pin(async move { ready.reads.available_permits() == 1 })
    })
    .await;
    market_read_backend(&state).await;
    read.close().await;
    observer.close().await;
}

#[tokio::test]
async fn market_read_cancellation_keeps_the_owner_until_the_transaction_ends() {
    let (state, observer, read) = state_with_slots(1).await;
    let release = Arc::new(tokio::sync::Notify::new());
    let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(1);
    let caller = {
        let state = state.clone();
        let release = release.clone();
        tokio::spawn(async move {
            run_market_read(&state, move |connection| {
                Box::pin(async move {
                    let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(&mut *connection)
                        .await
                        .map_err(|error| database_failure("probe", &error))?;
                    started_tx.send(backend).await.unwrap();
                    release.notified().await;
                    Ok(backend)
                })
            })
            .await
        })
    };
    let backend = started_rx.recv().await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    // The detached owner keeps the slot and its open read transaction.
    assert_eq!(state.reads.available_permits(), 0);
    let activity: String = sqlx::query_scalar("SELECT state FROM pg_stat_activity WHERE pid = $1")
        .bind(backend)
        .fetch_one(&observer)
        .await
        .unwrap();
    assert_eq!(activity, "idle in transaction");
    let refused = run_market_read(&state, |_| Box::pin(async { Ok(()) }))
        .await
        .unwrap_err();
    assert_market_read_error(&refused, 503, "READ_UNAVAILABLE", ApiErrorKind::Expected);

    // The owner ends the transaction, returns the connection and only then
    // releases the slot.
    release.notify_one();
    let ready = state.clone();
    wait_for("the owner to release admission", move || {
        let ready = ready.clone();
        Box::pin(async move { ready.reads.available_permits() == 1 })
    })
    .await;
    assert_eq!(market_read_backend(&state).await, backend);
    read.close().await;
    observer.close().await;
}

#[tokio::test]
async fn mounted_market_read_uses_the_request_admission_slot() {
    use axum::body::{Body, to_bytes};
    use tower::ServiceExt;
    let (state, observer, read) = state_with_slots(1).await;
    let handler_state = state.clone();
    let app = verdaxis_read::runtime::http::middleware_with_state(
        axum::Router::new().route(
            "/api/availability",
            axum::routing::get(move || {
                let state = handler_state.clone();
                async move {
                    run_market_read(&state, |connection| {
                        Box::pin(async move {
                            sqlx::query_scalar::<_, String>(
                                "SELECT current_setting('statement_timeout')",
                            )
                            .fetch_one(connection)
                            .await
                            .map_err(|error| database_failure("probe", &error))
                        })
                    })
                    .await
                    .map(axum::Json)
                }
            }),
        ),
        (*state).clone(),
    );
    // One slot: the read succeeds only by reusing the request's permit.
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/availability")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        r#""5s""#
    );
    assert_eq!(state.reads.available_permits(), 1);
    read.close().await;
    observer.close().await;
}

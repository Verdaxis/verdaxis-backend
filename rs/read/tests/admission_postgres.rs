//! Cancellation and capacity ownership against disposable PostgreSQL.
//!
//! Requires a disposable database and roles; never targets deployed databases.
use std::{sync::Arc, time::Duration};
use verdaxis_read::{AppState, Config, MarketPolicy, compiled_sha, connect, run_admitted};

const SLOW: &str = "SELECT pg_sleep(5)";

/// Build the admitted state and keep the readiness pool as an out-of-band
/// observer that never consumes an admission slot.
async fn state() -> (Arc<AppState>, sqlx::PgPool) {
    let config = Config {
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var("RUST_TEST_DATABASE_URL").expect("disposable URL required"),
        bind: "127.0.0.1:0".parse().unwrap(),
        pool_max: 9,
        python_budget: 52,
        configured_max_connections: 100,
    };
    verdaxis_read::validate_config(&config).expect("valid disposable configuration");
    let pools = connect(&config).await.expect("connect");
    let observer = pools.health.clone();
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
    (state.into(), observer)
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
    let (state, observer) = state().await;
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
                                    .map_err(|_| verdaxis_read::ApiError::internal())?;
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

    // Failed work closes its connection instead of returning it to the pool.
    assert!(
        run_admitted(&state, |connection| Box::pin(async move {
            sqlx::query("SELECT 1/0")
                .execute(connection)
                .await
                .map_err(|_| verdaxis_read::ApiError::internal())?;
            Ok(())
        }))
        .await
        .is_err()
    );
    assert_eq!(state.reads.available_permits(), 8);
    observer.close().await;
}

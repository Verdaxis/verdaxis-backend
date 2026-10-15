use std::{sync::Arc, time::Duration};
use verdaxis_read::{AppState, Config, MarketPolicy, compiled_sha, connect, run_admitted};

#[tokio::test]
async fn cancelled_query_keeps_slot_until_drop() {
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
    let pools = connect(&config).await.unwrap_or_else(|_| panic!("connect"));
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
    .unwrap_or_else(|_| panic!("state"));
    let state = Arc::new(state);
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
                                sqlx::query("SELECT pg_sleep(5)")
                                    .execute(connection)
                                    .await
                                    .map_err(|_| verdaxis_read::ApiError::internal())?;
                                Ok(())
                            })
                        })
                        .await
                        .map(|_| axum::Json(serde_json::json!({"ok":true})))
                    }
                }
            }),
        ),
        (*state).clone(),
    );
    use tower::ServiceExt;
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
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let active: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE usename = current_user AND query = 'SELECT pg_sleep(5)' AND state = 'active'").fetch_one(&observer).await.unwrap();
                if active == 8 { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("all eight reads started");
        let rejected = run_admitted(&state, |_| Box::pin(async { Ok(()) }))
            .await
            .expect_err("ninth rejected");
        assert_eq!(rejected.status, 503);
        assert_eq!(rejected.retry_after, Some(1));
        if cycle == 0 {
            for caller in callers {
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
            }
            assert_eq!(state.reads.available_permits(), 0);
        } else {
            for caller in callers {
                assert_eq!(caller.await.unwrap().status(), 200);
            }
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_stat_activity WHERE usename = current_user",
                )
                .fetch_one(&observer)
                .await
                .unwrap();
                assert!(count <= 9, "app-role connections: {count}");
                if state.reads.available_permits() == 8 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("work releases admission without acquire timeout");
    }
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

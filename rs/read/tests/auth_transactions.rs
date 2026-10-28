use verdaxis_read::auth::{AuthenticatedRequest, UserLockMode};

#[tokio::test]
async fn postgres_authentication_ownership_and_revocation() {
    use std::{sync::Arc, time::Duration};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        sync::{Barrier, Notify},
    };
    use verdaxis_read::auth::{
        AuthError, AuthenticatedError, VerificationSettings, run_authenticated, verify_access,
    };
    use verdaxis_read::{AppState, Config, MarketPolicy};
    let Ok(url) = std::env::var("AUTH_INTEROP_APP_URL") else {
        assert!(
            std::env::var("REQUIRE_AUTH_INTEROP").is_err(),
            "auth interop URL required"
        );
        return;
    };
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("AUTH_INTEROP_ADMIN_URL").unwrap())
        .await
        .unwrap();
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
    };
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let health = pools.health.clone();
    let read_pool = pools.read.clone();
    let state = Arc::new(
        AppState::new(
            config.clone(),
            pools,
            MarketPolicy {
                source_sha: config.release_sha.clone(),
                products: vec![],
                points: vec![],
            },
        )
        .unwrap(),
    );
    assert_eq!(health.options().get_max_connections(), 1);
    assert_eq!(read_pool.options().get_max_connections(), 16);
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/auth-vectors.json"
    ))
    .unwrap();
    let settings = VerificationSettings::new(
        fixture["settings"]["current"].as_str().unwrap().into(),
        None,
        fixture["settings"]["issuer"].as_str().unwrap().into(),
        fixture["settings"]["audience"].as_str().unwrap().into(),
    );
    let token = fixture["vectors"][0]["token"].as_str().unwrap();
    let access = || {
        verify_access(
            token,
            &settings,
            chrono::DateTime::from_timestamp(fixture["now"].as_i64().unwrap(), 0).unwrap(),
        )
        .unwrap()
    };
    sqlx::query("UPDATE users SET authentication_revoked_at=NULL,password_changed_at=NULL,status='APPROVED',email_verified=false,organization_id=NULL,must_change_password=false WHERE id='20000000-0000-4000-8000-000000000001'")
        .execute(&admin).await.unwrap();
    // The signed subject belongs to this seeded account; retain fixture signature.
    let subject = access().subject;
    sqlx::query("UPDATE users SET id=$1 WHERE id='20000000-0000-4000-8000-000000000001'")
        .bind(subject)
        .execute(&admin)
        .await
        .unwrap();
    // Two Rust shared reads enter together. PostgreSQL confirms an independent
    // account UPDATE is blocked until both authenticated owners finish.
    let barrier = Arc::new(Barrier::new(3));
    let release = Arc::new(Notify::new());
    let mut readers = vec![];
    for _ in 0..2 {
        let (state, barrier, release) = (state.clone(), barrier.clone(), release.clone());
        let admitted_access = access();
        readers.push(tokio::spawn(async move {
            run_authenticated(
                &state,
                admitted_access,
                AuthenticatedRequest::new("GET", "/api/notifications", false),
                move |tx| {
                    Box::pin(async move {
                        assert!(!tx.actor().email_verified);
                        assert!(tx.actor().organization_id.is_none());
                        let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
                            .fetch_one(tx.connection())
                            .await
                            .unwrap();
                        assert_eq!(read_only, "off");
                        barrier.wait().await;
                        release.notified().await;
                        Ok(())
                    })
                },
            )
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), barrier.wait())
        .await
        .unwrap();
    let python_blocked =
        tokio::process::Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or("python3".into()))
            .args(["-m", "tests.rust_contract.phase2.auth_writer"])
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .env("AUTH_INTEROP_SUBJECT", subject.to_string())
            .env("AUTH_INTEROP_EXPECT_BLOCKED", "true")
            .output()
            .await
            .unwrap();
    assert!(python_blocked.status.success());
    assert_eq!(
        std::str::from_utf8(&python_blocked.stdout).unwrap().trim(),
        "BLOCKED"
    );
    let mut writer = admin.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *writer)
        .await
        .unwrap();
    let blocked = sqlx::query("UPDATE users SET authentication_revoked_at=now() WHERE id=$1")
        .bind(subject)
        .execute(&mut *writer)
        .await
        .unwrap_err();
    assert_eq!(
        blocked.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    writer.rollback().await.unwrap();
    release.notify_waiters();
    for reader in readers {
        reader.await.unwrap().unwrap();
    }
    // Python takes the other lock order. Its explicit stdin barrier holds the
    // revocation transaction while Rust waits, then Rust reads current state.
    let mut python =
        tokio::process::Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or("python3".into()))
            .args(["-m", "tests.rust_contract.phase2.auth_writer"])
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .env("AUTH_INTEROP_SUBJECT", subject.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
    let mut output = BufReader::new(python.stdout.take().unwrap()).lines();
    assert_eq!(output.next_line().await.unwrap().unwrap(), "LOCKED");
    let state2 = state.clone();
    let stale_access = access();
    let stale = tokio::spawn(async move {
        run_authenticated(
            &state2,
            stale_access,
            AuthenticatedRequest::new("POST", "/api/feedback", false),
            |_| {
                Box::pin(async {
                    panic!("stale request reached work");
                    #[allow(unreachable_code)]
                    Ok(())
                })
            },
        )
        .await
    });
    // Observe PostgreSQL waiting on the current-user lock before releasing
    // Python; spawning a future alone would not prove the second lock order.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE usename='verdaxis_phase2_app_test' AND wait_event_type='Lock' AND query LIKE '%FROM users WHERE id=$1 FOR UPDATE%'")
                .fetch_one(&admin).await.unwrap();
            if waiting > 0 { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    python
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"COMMIT\n")
        .await
        .unwrap();
    assert!(python.wait().await.unwrap().success());
    assert!(matches!(
        stale.await.unwrap(),
        Err(AuthenticatedError::Authentication(
            AuthError::SessionRevoked
        ))
    ));
    sqlx::query("UPDATE users SET authentication_revoked_at=NULL WHERE id=$1")
        .bind(subject)
        .execute(&admin)
        .await
        .unwrap();
    // Cancel eight clients only after their work is admitted. Detached owners
    // keep permits until the controlled DB work is released and drained.
    let entered = Arc::new(Barrier::new(9));
    let drain = Arc::new(Notify::new());
    let mut clients = vec![];
    for _ in 0..8 {
        let (s, b, n) = (state.clone(), entered.clone(), drain.clone());
        let admitted_access = access();
        clients.push(tokio::spawn(async move {
            run_authenticated(
                &s,
                admitted_access,
                AuthenticatedRequest::new("GET", "/api/notifications", false),
                move |tx| {
                    Box::pin(async move {
                        sqlx::query("SELECT 1")
                            .execute(tx.connection())
                            .await
                            .unwrap();
                        b.wait().await;
                        n.notified().await;
                        Ok(())
                    })
                },
            )
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), entered.wait())
        .await
        .unwrap();
    for client in clients {
        client.abort();
    }
    assert_eq!(state.reads.available_permits(), 8);
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&health)
            .await
            .unwrap(),
        1
    );
    drain.notify_waiters();
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.reads.available_permits() != 16 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Every returned data connection retains the public-read session default.
    let default_mode: String = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(&read_pool)
        .await
        .unwrap();
    assert_eq!(default_mode, "on");
    read_pool.close().await;
    health.close().await;
    admin.close().await;
}

#[test]
fn only_audited_get_templates_share_the_user_lock() {
    for path in [
        "/api/notifications",
        "/api/notifications/unread-count",
        "/api/users/me/preferences",
        "/api/watchlists",
        "/api/watchlists/{watchlist_id}",
        "/api/watchlists/{watchlist_id}/events",
    ] {
        assert_eq!(
            AuthenticatedRequest::new("GET", path, false).lock_mode(),
            UserLockMode::Share
        );
        assert_eq!(
            AuthenticatedRequest::new("GET", path, true).lock_mode(),
            UserLockMode::Update
        );
        assert_eq!(
            AuthenticatedRequest::new("POST", path, false).lock_mode(),
            UserLockMode::Update
        );
    }
    for path in [
        "/api/alerts",
        "/api/referrals/my-referrals",
        "/api/admin/feedback",
        "/api/future",
    ] {
        assert_eq!(
            AuthenticatedRequest::new("GET", path, false).lock_mode(),
            UserLockMode::Update
        );
    }
}

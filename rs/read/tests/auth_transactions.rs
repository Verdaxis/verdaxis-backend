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
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
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
    // Exercise the actual public composition against the migrated disposable DB.
    {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let root = std::env::temp_dir().join(format!("composed-fleet-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        std::fs::write(
            root.join("data/fleet.json"),
            br#"{"entries":[],"last_updated":"2024-01-01","sources":[]}"#,
        )
        .unwrap();
        let mut composed = (*state).clone();
        let mut config = (*composed.config).clone();
        config.fleet_artifact_path = verdaxis_read::runtime::config::resolve_fleet_artifact_path(
            std::path::Path::new("data/fleet.json"),
            &root,
        )
        .unwrap();
        composed.config = Arc::new(config);
        let app = verdaxis_read::router(composed);
        for path in [
            "/api/benchmarks",
            "/api/availability",
            "/api/demand",
            "/api/producers",
            "/api/fleet-intelligence",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
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
    // Compliance limits are transaction-local and do not change other reads.
    for (method, path, expected_lock, expected_statement) in [
        ("GET", "/api/compliance/fleet", "500ms", "5s"),
        (
            "GET",
            "/api/compliance/vessels/{vessel_id}/score",
            "500ms",
            "5s",
        ),
        ("POST", "/api/compliance/scenario", "500ms", "5s"),
        ("POST", "/api/compliance/pricing-overlay", "500ms", "5s"),
        ("GET", "/api/notifications", "3s", "30s"),
        ("GET", "/api/compliance/scenario", "3s", "30s"),
        ("POST", "/api/compliance/scenario/extra", "3s", "30s"),
    ] {
        run_authenticated(
            &state,
            access(),
            AuthenticatedRequest::new(method, path, false),
            move |tx| {
                Box::pin(async move {
                    let lock: String = sqlx::query_scalar("SHOW lock_timeout")
                        .fetch_one(tx.connection())
                        .await
                        .unwrap();
                    let statement: String = sqlx::query_scalar("SHOW statement_timeout")
                        .fetch_one(tx.connection())
                        .await
                        .unwrap();
                    assert_eq!(lock, expected_lock, "{method} {path}");
                    assert_eq!(statement, expected_statement, "{method} {path}");
                    Ok(())
                })
            },
        )
        .await
        .unwrap();
    }
    // A held actor row must expire before the ordinary three-second lock
    // deadline. The callback must not run before current-account admission.
    let mut actor_lock = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(subject)
        .execute(&mut *actor_lock)
        .await
        .unwrap();
    let refused = tokio::time::timeout(
        Duration::from_secs(2),
        run_authenticated(
            &state,
            access(),
            AuthenticatedRequest::new("GET", "/api/compliance/fleet", false),
            |_| {
                Box::pin(async {
                    panic!("locked actor reached compliance work");
                    #[allow(unreachable_code)]
                    Ok(())
                })
            },
        ),
    )
    .await
    .expect("compliance lock deadline was not set before actor lookup");
    assert!(matches!(refused, Err(AuthenticatedError::Database(_))));
    actor_lock.rollback().await.unwrap();
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
    let python_blocked = tokio::process::Command::new(
        std::env::var("RUST_TEST_PYTHON").unwrap_or("venv/bin/python".into()),
    )
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
    let mut python = tokio::process::Command::new(
        std::env::var("RUST_TEST_PYTHON").unwrap_or("venv/bin/python".into()),
    )
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
fn compliance_timeouts_select_only_exact_method_and_template_pairs() {
    for (method, path) in [
        ("GET", "/api/compliance/fleet"),
        ("GET", "/api/compliance/vessels/{vessel_id}/score"),
        ("POST", "/api/compliance/scenario"),
        ("POST", "/api/compliance/pricing-overlay"),
    ] {
        for support in [false, true] {
            let request = AuthenticatedRequest::new(method, path, support);
            assert!(request.scoped_read_timeouts(), "{method} {path}");
            assert_eq!(request.lock_mode(), UserLockMode::Update);
        }
        let other_method = if method == "GET" { "POST" } else { "GET" };
        assert!(!AuthenticatedRequest::new(other_method, path, false).scoped_read_timeouts());
        assert!(
            !AuthenticatedRequest::new(method, &format!("{path}/extra"), false)
                .scoped_read_timeouts()
        );
    }
    assert!(!AuthenticatedRequest::new("GET", "/api/notifications", false).scoped_read_timeouts());
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

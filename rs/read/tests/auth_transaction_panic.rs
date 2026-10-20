//! A panic between BEGIN and COMMIT must not return the connection to the pool
//! inside the abandoned transaction. The switched-in sqlx transaction guard
//! queues a ROLLBACK, so the next user of the connection starts clean and the
//! panicked request's rows and locks are gone.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use chrono::DateTime;
use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;
use verdaxis_read::{
    ApiError, AppState, Config, MarketPolicy,
    auth::{
        AuthenticatedError, AuthenticatedRequest, VerificationSettings, run_authenticated,
        verify_access,
    },
};

/// Both cases share the fixture subject and the single read connection.
static SERIAL: Mutex<()> = Mutex::const_new(());

struct Fixture {
    state: Arc<AppState>,
    read_pool: sqlx::PgPool,
    admin: sqlx::PgPool,
    subject: Uuid,
}

fn access() -> Option<verdaxis_read::auth::VerifiedAccess> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/auth-vectors.json"
    ))
    .unwrap();
    let settings = VerificationSettings::new(
        fixture["settings"]["current"].as_str().unwrap().into(),
        None,
        fixture["settings"]["issuer"].as_str().unwrap().into(),
        fixture["settings"]["audience"].as_str().unwrap().into(),
    );
    verify_access(
        fixture["vectors"][0]["token"].as_str().unwrap(),
        &settings,
        DateTime::from_timestamp(fixture["now"].as_i64().unwrap(), 0).unwrap(),
    )
    .ok()
}

async fn fixture() -> Option<Fixture> {
    let Ok(url) = std::env::var("AUTH_INTEROP_APP_URL") else {
        assert!(
            std::env::var("REQUIRE_AUTH_INTEROP").is_err(),
            "auth interop URL required"
        );
        return None;
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
        // One admission slot means a one-connection read pool. The panicking
        // request must give that one connection back in a clean state.
        read_admission_slots: 1,
        pool_max: 2,
        python_budget: 52,
        configured_max_connections: 100,
    };
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let read_pool = pools.read.clone();
    assert_eq!(read_pool.options().get_max_connections(), 1);
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
    let access = access().unwrap();
    let subject = access.subject;
    // The other auth tests share this seeded account. Move it onto the token
    // subject (idempotent when a previous test already did) and reset it to the
    // baseline every auth test expects, whatever order the test targets run in.
    sqlx::query("UPDATE users SET id=$1 WHERE id='20000000-0000-4000-8000-000000000001'")
        .bind(subject)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE users SET status='APPROVED',organization_id=NULL,\
         email_verified=false,must_change_password=false,password_changed_at=NULL,\
         authentication_revoked_at=NULL WHERE id=$1",
    )
    .bind(subject)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("DELETE FROM watchlists WHERE user_id=$1")
        .bind(subject)
        .execute(&admin)
        .await
        .unwrap();
    Some(Fixture {
        state,
        read_pool,
        admin,
        subject,
    })
}

fn assert_internal<T: std::fmt::Debug>(outcome: Result<T, AuthenticatedError>) {
    match outcome {
        Err(AuthenticatedError::Database(error)) => {
            assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.kind, verdaxis_read::ApiErrorKind::Unexpected);
        }
        other => panic!("expected an internal error, got {other:?}"),
    }
}

async fn insert_watchlist(
    state: &AppState,
    subject: Uuid,
    id: Uuid,
) -> Result<(), AuthenticatedError> {
    run_authenticated(
        state,
        access().unwrap(),
        AuthenticatedRequest::new("POST", "/api/watchlists", false),
        move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO watchlists (id,user_id,name,kind) VALUES ($1,$2,'panic','CUSTOM')",
                )
                .bind(id)
                .bind(subject)
                .execute(tx.connection())
                .await
                .map_err(|_| ApiError::database())?;
                Ok(())
            })
        },
    )
    .await
}

async fn count_watchlists(admin: &sqlx::PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM watchlists WHERE id=$1")
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_panic_inside_work_rolls_back_before_the_next_request() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = fixture().await else {
        return;
    };
    let panicked = Uuid::new_v4();
    let outcome = run_authenticated(
        &fixture.state,
        access().unwrap(),
        AuthenticatedRequest::new("POST", "/api/watchlists", false),
        move |tx| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO watchlists (id,user_id,name,kind) VALUES ($1,$2,'panic','CUSTOM')",
                )
                .bind(panicked)
                .bind(fixture.subject)
                .execute(tx.connection())
                .await
                .map_err(|_| ApiError::database())?;
                panic!("panic after a write inside the authenticated transaction");
                #[allow(unreachable_code)]
                Ok(())
            })
        },
    )
    .await;
    assert_internal(outcome);

    // The one pooled connection serves the next request. If the panicked
    // transaction leaked, this COMMIT would also commit the row above.
    let committed = Uuid::new_v4();
    insert_watchlist(&fixture.state, fixture.subject, committed)
        .await
        .unwrap();
    assert_eq!(count_watchlists(&fixture.admin, panicked).await, 0);
    assert_eq!(count_watchlists(&fixture.admin, committed).await, 1);

    // The next user of the connection runs under the read-only session
    // default, not inside the abandoned read-write transaction.
    let mut connection = fixture.read_pool.acquire().await.unwrap();
    let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
    let still_open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE pid=pg_backend_pid() AND state='idle in transaction'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(still_open, 0);
    drop(connection);

    // The user-row lock is free: a second session can take it without waiting.
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(fixture.subject)
        .fetch_one(&fixture.admin)
        .await
        .unwrap();

    sqlx::query("DELETE FROM watchlists WHERE user_id=$1")
        .bind(fixture.subject)
        .execute(&fixture.admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_panic_in_the_account_row_decode_rolls_back_before_the_next_request() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = fixture().await else {
        return;
    };
    // PostgreSQL `infinity` does not fit a chrono timestamp. The `row.get`
    // call for `password_changed_at` unwinds before `work` starts.
    sqlx::query("UPDATE users SET password_changed_at='infinity' WHERE id=$1")
        .bind(fixture.subject)
        .execute(&fixture.admin)
        .await
        .unwrap();
    let work_entered = Arc::new(AtomicBool::new(false));
    let entered = work_entered.clone();
    let outcome = run_authenticated(
        &fixture.state,
        access().unwrap(),
        AuthenticatedRequest::new("POST", "/api/watchlists", false),
        move |_| {
            Box::pin(async move {
                entered.store(true, Ordering::SeqCst);
                Ok(())
            })
        },
    )
    .await;
    assert_internal(outcome);
    assert!(!work_entered.load(Ordering::SeqCst));

    // Reusing the one pooled connection first sends the queued ROLLBACK. It
    // must report the read-only session default and release the row lock.
    let mut connection = fixture.read_pool.acquire().await.unwrap();
    let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
    drop(connection);
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(fixture.subject)
        .fetch_one(&fixture.admin)
        .await
        .unwrap();

    // The row still holds the undecodable value. Repair it, then prove the
    // next request commits normally.
    sqlx::query("UPDATE users SET password_changed_at=NULL WHERE id=$1")
        .bind(fixture.subject)
        .execute(&fixture.admin)
        .await
        .unwrap();
    let committed = Uuid::new_v4();
    insert_watchlist(&fixture.state, fixture.subject, committed)
        .await
        .unwrap();
    assert_eq!(count_watchlists(&fixture.admin, committed).await, 1);
    sqlx::query("DELETE FROM watchlists WHERE user_id=$1")
        .bind(fixture.subject)
        .execute(&fixture.admin)
        .await
        .unwrap();
}

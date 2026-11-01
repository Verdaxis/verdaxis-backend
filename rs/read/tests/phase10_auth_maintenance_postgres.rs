//! Auth maintenance job: bounded cleanup and approval-mail retries.
//!
//! Every case runs against a disposable PostgreSQL in its own schema. Races use
//! independent connections; a contender is released only after PostgreSQL
//! reports it blocked or after it has finished, never after a sleep.
#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;
use verdaxis_read::business::approval_mail::{
    ApprovalMailBatchSize, FrozenApprovalPayload, claim_approval_mail, finalize_approval_mail,
};
use verdaxis_read::cli::jobs::auth::{
    AuthMaintenanceReport, CleanupBatchSize, CleanupCounts, DEFAULT_BATCH_SIZE, MAX_BATCH_SIZE,
    clean_expired_auth_state, execute, run,
};
use verdaxis_read::cli::mail::approval::{ApprovalOutcome, deliver_due, retry_due};
use verdaxis_read::cli::{Environment, Exit, IdentityArgs, JobArgs, JobKind};

const SCHEMA_SQL: &str = include_str!("fixtures/phase10_auth_maintenance_schema.sql");
/// Upper bound for a contender to reach its lock or its provider call. It only
/// bounds a failing case; a passing case never waits for it.
const STEP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

fn instant() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-01T09:00:00.123456Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn token(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

fn payload(recipient: &str) -> Value {
    json!({
        "from": "sender@example.test",
        "to": [recipient],
        "subject": "Synthetic approval",
        "html": "<p>Synthetic</p>",
    })
}

struct Fixture {
    admin: PgConnection,
    runtime: String,
    schema: String,
    pool: PgPool,
}

impl Fixture {
    async fn new() -> Self {
        let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL required");
        let runtime = std::env::var("RUST_TEST_DATABASE_URL")
            .expect("explicit disposable runtime URL required");
        let m = url::Url::parse(&migrator).unwrap();
        let r = url::Url::parse(&runtime).unwrap();
        assert_eq!(m.host_str(), Some("127.0.0.1"));
        assert_eq!(r.host_str(), m.host_str());
        assert_eq!(r.port(), m.port());
        assert_eq!(r.path(), m.path());
        assert!(m.path().ends_with("_test"));
        assert_eq!(r.username(), "rust_app");
        let mut admin = PgConnection::connect(&migrator).await.unwrap();
        let schema = format!("p10_auth_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}; {SCHEMA_SQL} \
             GRANT USAGE ON SCHEMA {schema} TO rust_app;"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let pool = runtime_pool(&runtime, &schema, 4).await;
        Self {
            admin,
            runtime,
            schema,
            pool,
        }
    }

    /// A second job process: its own pool and its own connections.
    async fn second_pool(&self) -> PgPool {
        runtime_pool(&self.runtime, &self.schema, 4).await
    }

    /// One independent application-role connection, for a contender.
    async fn connection(&self) -> PgConnection {
        let mut conn = PgConnection::connect(&self.runtime).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "SET search_path TO {}",
            self.schema
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        conn
    }

    async fn user(&mut self, id: Uuid) -> Uuid {
        sqlx::query("INSERT INTO users(id,email) VALUES($1,$2)")
            .bind(id)
            .bind(format!("{}@example.test", id.simple()))
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    async fn set_reset(&mut self, user: Uuid, hash: &str, expires: Option<DateTime<Utc>>) {
        sqlx::query(
            "UPDATE users SET password_reset_token_hash=$2,password_reset_expires=$3 WHERE id=$1",
        )
        .bind(user)
        .bind(hash)
        .bind(expires)
        .execute(&mut self.admin)
        .await
        .unwrap();
    }

    async fn set_verification(&mut self, user: Uuid, hash: &str, expires: Option<DateTime<Utc>>) {
        sqlx::query(
            "UPDATE users SET email_verification_token_hash=$2,\
             email_verification_token_expires_at=$3 WHERE id=$1",
        )
        .bind(user)
        .bind(hash)
        .bind(expires)
        .execute(&mut self.admin)
        .await
        .unwrap();
    }

    async fn session(
        &mut self,
        user: Uuid,
        family: Uuid,
        revoked: bool,
        created_at: Option<DateTime<Utc>>,
        expires_at: DateTime<Utc>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO refresh_sessions(id,user_id,family_id,jti_hash,revoked,created_at,expires_at) \
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(id)
        .bind(user)
        .bind(family)
        .bind(format!("{}{}", id.simple(), id.simple()))
        .bind(revoked)
        .bind(created_at)
        .bind(expires_at)
        .execute(&mut self.admin)
        .await
        .unwrap();
        id
    }

    async fn registration(
        &mut self,
        expires_at: DateTime<Utc>,
        used_at: Option<DateTime<Utc>>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO pending_registrations(id,token_hash,email,password_hash,expires_at,used_at) \
             VALUES($1,$2,$3,'synthetic',$4,$5)",
        )
        .bind(id)
        .bind(format!("{}{}", id.simple(), id.simple()))
        .bind(format!("{}@example.test", id.simple()))
        .bind(expires_at)
        .bind(used_at)
        .execute(&mut self.admin)
        .await
        .unwrap();
        id
    }

    /// Queue one frozen approval mail exactly as the approval route does.
    async fn queue_mail(&mut self, user: Uuid, retry_at: DateTime<Utc>) -> Uuid {
        let transition = Uuid::new_v4();
        sqlx::query(
            "UPDATE users SET pending_approval_email_transition_id=$2,\
             pending_approval_email_payload=$3,pending_approval_email_retry_at=$4 WHERE id=$1",
        )
        .bind(user)
        .bind(transition)
        .bind(payload(&recipient(user)))
        .bind(retry_at)
        .execute(&mut self.admin)
        .await
        .unwrap();
        transition
    }

    async fn mail_state(
        &mut self,
        user: Uuid,
    ) -> (Option<Uuid>, Option<Value>, Option<DateTime<Utc>>) {
        sqlx::query_as(
            "SELECT pending_approval_email_transition_id,pending_approval_email_payload,\
             pending_approval_email_retry_at FROM users WHERE id=$1",
        )
        .bind(user)
        .fetch_one(&mut self.admin)
        .await
        .unwrap()
    }

    async fn tokens(
        &mut self,
        user: Uuid,
    ) -> (
        Option<String>,
        Option<DateTime<Utc>>,
        Option<String>,
        Option<DateTime<Utc>>,
    ) {
        sqlx::query_as(
            "SELECT password_reset_token_hash,password_reset_expires,\
             email_verification_token_hash,email_verification_token_expires_at \
             FROM users WHERE id=$1",
        )
        .bind(user)
        .fetch_one(&mut self.admin)
        .await
        .unwrap()
    }

    async fn exists(&mut self, table: &str, id: Uuid) -> bool {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=$1)"
        )))
        .bind(id)
        .fetch_one(&mut self.admin)
        .await
        .unwrap()
    }

    async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.admin)
        .await
        .unwrap();
    }
}

async fn runtime_pool(runtime: &str, schema: &str, connections: u32) -> PgPool {
    let path = format!("SET search_path TO {schema}");
    PgPoolOptions::new()
        .max_connections(connections)
        .after_connect(move |conn, _| {
            let path = path.clone();
            Box::pin(async move {
                sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(runtime)
        .await
        .unwrap()
}

fn recipient(user: Uuid) -> String {
    format!("recipient-{}@example.test", user.simple())
}

async fn backend_pid(conn: &mut PgConnection) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(conn)
        .await
        .unwrap()
}

/// Wait until the task finished or PostgreSQL reports a backend blocked by
/// `holder`. Returns true when something waited on the holder's locks.
async fn blocked_or_finished<T>(
    watcher: &mut PgConnection,
    holder: i32,
    task: &JoinHandle<T>,
) -> bool {
    let deadline = tokio::time::Instant::now() + STEP_DEADLINE;
    loop {
        if task.is_finished() {
            return false;
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
             AND $1 = ANY(pg_blocking_pids(pid)))",
        )
        .bind(holder)
        .fetch_one(&mut *watcher)
        .await
        .unwrap();
        if blocked {
            return true;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the contender neither finished nor blocked"
        );
        tokio::task::yield_now().await;
    }
}

type SendFuture = Pin<Box<dyn Future<Output = bool> + Send>>;

/// A fake provider that records each (recipient, idempotency key) it accepts
/// or refuses. Nothing leaves the process.
#[derive(Clone, Default)]
struct Provider {
    sends: Arc<Mutex<Vec<(String, String)>>>,
}

impl Provider {
    fn sender(
        &self,
        accepted: bool,
    ) -> impl Fn(FrozenApprovalPayload, String) -> SendFuture + Send + Sync + 'static {
        let sends = self.sends.clone();
        move |payload, key| {
            sends.lock().unwrap().push((payload.to[0].clone(), key));
            Box::pin(async move { accepted })
        }
    }

    /// Accepts after the test releases it; signals when the call has started.
    fn gated(
        &self,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    ) -> impl Fn(FrozenApprovalPayload, String) -> SendFuture + Send + Sync + 'static {
        let sends = self.sends.clone();
        move |payload, key| {
            sends.lock().unwrap().push((payload.to[0].clone(), key));
            let entered = entered.clone();
            let release = release.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                true
            })
        }
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.sends.lock().unwrap().clone()
    }
}

async fn entered(signal: &Notify) {
    tokio::time::timeout(STEP_DEADLINE, signal.notified())
        .await
        .expect("the provider call started");
}

fn key(transition: Uuid) -> String {
    format!("account-approval/{transition}")
}

fn batch(size: u32) -> CleanupBatchSize {
    CleanupBatchSize::new(size).unwrap()
}

fn mail_batch(size: u32) -> ApprovalMailBatchSize {
    ApprovalMailBatchSize::new(size).unwrap()
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cleanup_deletes_expired_refresh_sessions_without_successor() {
    let mut f = Fixture::new().await;
    let now = Utc::now();
    let a = f.user(Uuid::new_v4()).await;
    let b = f.user(Uuid::new_v4()).await;
    let c = f.user(Uuid::new_v4()).await;
    let old = Some(now - Duration::days(2));

    // Refresh sessions, one family per case.
    let expired = f
        .session(a, Uuid::new_v4(), false, old, now - Duration::seconds(1))
        .await;
    let live_family = Uuid::new_v4();
    let revoked_with_successor = f
        .session(a, live_family, true, old, now + Duration::days(1))
        .await;
    let successor = f
        .session(a, live_family, false, Some(now), now + Duration::days(1))
        .await;
    let revoked_without_successor = f
        .session(a, Uuid::new_v4(), true, old, now + Duration::days(1))
        .await;
    // An expired member is not a live successor; both rows go.
    let dead_family = Uuid::new_v4();
    let revoked_with_expired_member = f
        .session(a, dead_family, true, old, now + Duration::days(1))
        .await;
    let expired_member = f
        .session(a, dead_family, false, old, now - Duration::hours(1))
        .await;
    let recently_revoked = f
        .session(
            a,
            Uuid::new_v4(),
            true,
            Some(now - Duration::hours(1)),
            now + Duration::days(1),
        )
        .await;
    let revoked_without_creation_time = f
        .session(a, Uuid::new_v4(), true, None, now + Duration::days(1))
        .await;

    // Pending registrations.
    let expired_registration = f.registration(now - Duration::seconds(1), None).await;
    let consumed_registration = f.registration(now + Duration::minutes(5), Some(now)).await;
    let live_registration = f.registration(now + Duration::minutes(5), None).await;

    // Token digests: no expiry and a passed expiry are cleared; a future one stays.
    f.set_reset(a, &token(1), None).await;
    f.set_reset(b, &token(2), Some(now - Duration::seconds(1)))
        .await;
    f.set_reset(c, &token(3), Some(now + Duration::hours(1)))
        .await;
    f.set_verification(a, &token(4), None).await;
    f.set_verification(b, &token(5), Some(now - Duration::seconds(1)))
        .await;
    f.set_verification(c, &token(6), Some(now + Duration::days(1)))
        .await;
    let transition = f.queue_mail(a, now + Duration::hours(1)).await;

    let counts = clean_expired_auth_state(&f.pool, now, CleanupBatchSize::default())
        .await
        .unwrap();
    assert_eq!(
        counts,
        CleanupCounts {
            refresh_sessions_deleted: 4,
            pending_registrations_deleted: 2,
            password_reset_hashes_cleared: 2,
            email_verification_hashes_cleared: 2,
        }
    );
    for gone in [
        expired,
        revoked_without_successor,
        revoked_with_expired_member,
        expired_member,
    ] {
        assert!(!f.exists("refresh_sessions", gone).await);
    }
    for kept in [
        revoked_with_successor,
        successor,
        recently_revoked,
        revoked_without_creation_time,
    ] {
        assert!(f.exists("refresh_sessions", kept).await);
    }
    assert!(
        !f.exists("pending_registrations", expired_registration)
            .await
    );
    assert!(
        !f.exists("pending_registrations", consumed_registration)
            .await
    );
    assert!(f.exists("pending_registrations", live_registration).await);
    assert_eq!(f.tokens(a).await, (None, None, None, None));
    assert_eq!(f.tokens(b).await, (None, None, None, None));
    let kept = f.tokens(c).await;
    assert_eq!(kept.0.as_deref(), Some(token(3).as_str()));
    assert_eq!(kept.2.as_deref(), Some(token(6).as_str()));
    // Cleanup never touches the approval-mail columns.
    assert_eq!(f.mail_state(a).await.0, Some(transition));

    // A second run has nothing left to do.
    let again = clean_expired_auth_state(&f.pool, now, CleanupBatchSize::default())
        .await
        .unwrap();
    assert_eq!(again, CleanupCounts::default());
    f.finish().await;
}

#[tokio::test]
async fn cleanup_batches_are_bounded_per_category_in_id_order() {
    assert_eq!(CleanupBatchSize::default().get(), DEFAULT_BATCH_SIZE);
    assert_eq!(DEFAULT_BATCH_SIZE, 1000);
    assert_eq!(MAX_BATCH_SIZE, 10_000);
    assert_eq!(CleanupBatchSize::new(0), Err(Exit::Usage));
    assert_eq!(CleanupBatchSize::new(10_001), Err(Exit::Usage));
    assert_eq!(CleanupBatchSize::new(10_000).unwrap().get(), 10_000);
    assert_eq!(CleanupBatchSize::new(1).unwrap().get(), 1);

    let mut f = Fixture::new().await;
    let now = Utc::now();
    let users = [
        Uuid::from_u128(0x1000),
        Uuid::from_u128(0x2000),
        Uuid::from_u128(0x3000),
    ];
    for (seed, user) in users.into_iter().enumerate() {
        f.user(user).await;
        let seed = u8::try_from(seed).unwrap();
        f.set_reset(user, &token(10 + seed), Some(now - Duration::hours(1)))
            .await;
        f.set_verification(user, &token(20 + seed), Some(now - Duration::hours(1)))
            .await;
        f.session(user, Uuid::new_v4(), false, None, now - Duration::hours(1))
            .await;
        f.registration(now - Duration::hours(1), None).await;
    }

    let first = clean_expired_auth_state(&f.pool, now, batch(2))
        .await
        .unwrap();
    assert_eq!(
        first,
        CleanupCounts {
            refresh_sessions_deleted: 2,
            pending_registrations_deleted: 2,
            password_reset_hashes_cleared: 2,
            email_verification_hashes_cleared: 2,
        }
    );
    // Users are taken in id order, the order every ordered writer uses.
    assert_eq!(f.tokens(users[0]).await.0, None);
    assert_eq!(f.tokens(users[1]).await.0, None);
    assert_eq!(
        f.tokens(users[2]).await.0.as_deref(),
        Some(token(12).as_str())
    );

    let second = clean_expired_auth_state(&f.pool, now, batch(2))
        .await
        .unwrap();
    assert_eq!(
        second,
        CleanupCounts {
            refresh_sessions_deleted: 1,
            pending_registrations_deleted: 1,
            password_reset_hashes_cleared: 1,
            email_verification_hashes_cleared: 1,
        }
    );
    let third = clean_expired_auth_state(&f.pool, now, batch(2))
        .await
        .unwrap();
    assert_eq!(third, CleanupCounts::default());
    f.finish().await;
}

/// A request renews a user's expired verification and reset digests while it
/// holds that user's row lock. Cleanup runs meanwhile: it must neither wait for
/// that row nor clear the new digests once the renewal commits.
#[tokio::test]
async fn cleanup_does_not_clear_a_token_renewed_under_lock() {
    let mut f = Fixture::new().await;
    let now = Utc::now();
    let renewed = f.user(Uuid::new_v4()).await;
    let other = f.user(Uuid::new_v4()).await;
    f.set_verification(renewed, &token(1), Some(now - Duration::hours(1)))
        .await;
    f.set_reset(renewed, &token(2), Some(now - Duration::hours(1)))
        .await;
    f.set_verification(other, &token(3), Some(now - Duration::hours(1)))
        .await;

    let new_verification_expiry = now + Duration::days(1);
    let new_reset_expiry = now + Duration::hours(1);
    let mut renewal = f.connection().await;
    let renewal_pid = backend_pid(&mut renewal).await;
    sqlx::query("BEGIN").execute(&mut renewal).await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(renewed)
        .execute(&mut renewal)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE users SET email_verification_token_hash=$2,email_verification_token_expires_at=$3,\
         password_reset_token_hash=$4,password_reset_expires=$5 WHERE id=$1",
    )
    .bind(renewed)
    .bind(token(7))
    .bind(new_verification_expiry)
    .bind(token(8))
    .bind(new_reset_expiry)
    .execute(&mut renewal)
    .await
    .unwrap();

    let pool = f.pool.clone();
    let cleanup = tokio::spawn(async move {
        clean_expired_auth_state(&pool, now, CleanupBatchSize::default()).await
    });
    let mut watcher = f.connection().await;
    let waited = blocked_or_finished(&mut watcher, renewal_pid, &cleanup).await;
    sqlx::query("COMMIT").execute(&mut renewal).await.unwrap();
    let counts = cleanup.await.unwrap().unwrap();

    let (reset, reset_expiry, verification, verification_expiry) = f.tokens(renewed).await;
    assert_eq!(verification.as_deref(), Some(token(7).as_str()));
    assert_eq!(
        verification_expiry.map(|t| t.timestamp_micros()),
        Some(new_verification_expiry.timestamp_micros())
    );
    assert_eq!(reset.as_deref(), Some(token(8).as_str()));
    assert_eq!(
        reset_expiry.map(|t| t.timestamp_micros()),
        Some(new_reset_expiry.timestamp_micros())
    );
    assert!(!waited, "cleanup waited on a row another request held");
    // The run still did its other work.
    assert_eq!(counts.email_verification_hashes_cleared, 1);
    assert_eq!(counts.password_reset_hashes_cleared, 0);
    assert_eq!(f.tokens(other).await.2, None);

    // Later runs leave the renewed, unexpired digests alone.
    let again = clean_expired_auth_state(&f.pool, now, CleanupBatchSize::default())
        .await
        .unwrap();
    assert_eq!(again, CleanupCounts::default());
    assert_eq!(
        f.tokens(renewed).await.2.as_deref(),
        Some(token(7).as_str())
    );
    f.finish().await;
}

/// Rows held by another transaction (a refresh rotation, a registration, or an
/// organization write that locks its users in id order) are skipped, not
/// awaited. The next run removes them.
#[tokio::test]
async fn cleanup_skips_locked_rows_and_never_waits() {
    let mut f = Fixture::new().await;
    let now = Utc::now();
    let low = f.user(Uuid::from_u128(0x1000)).await;
    let high = f.user(Uuid::from_u128(0x2000)).await;
    for user in [low, high] {
        f.set_reset(user, &token(1), Some(now - Duration::hours(1)))
            .await;
    }
    let held_session = f
        .session(low, Uuid::new_v4(), false, None, now - Duration::hours(1))
        .await;
    let free_session = f
        .session(high, Uuid::new_v4(), false, None, now - Duration::hours(1))
        .await;
    let held_registration = f.registration(now - Duration::hours(1), None).await;

    let mut holder = f.connection().await;
    let holder_pid = backend_pid(&mut holder).await;
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    for (sql, id) in [
        ("SELECT id FROM users WHERE id=$1 FOR UPDATE", low),
        ("SELECT id FROM users WHERE id=$1 FOR UPDATE", high),
        (
            "SELECT id FROM refresh_sessions WHERE id=$1 FOR UPDATE",
            held_session,
        ),
        (
            "SELECT id FROM pending_registrations WHERE id=$1 FOR UPDATE",
            held_registration,
        ),
    ] {
        sqlx::query(sql)
            .bind(id)
            .execute(&mut holder)
            .await
            .unwrap();
    }

    let pool = f.pool.clone();
    let cleanup = tokio::spawn(async move {
        clean_expired_auth_state(&pool, now, CleanupBatchSize::default()).await
    });
    let mut watcher = f.connection().await;
    let waited = blocked_or_finished(&mut watcher, holder_pid, &cleanup).await;
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
    let counts = cleanup.await.unwrap().unwrap();
    assert!(!waited, "cleanup waited on a held row");
    assert_eq!(
        counts,
        CleanupCounts {
            refresh_sessions_deleted: 1,
            ..CleanupCounts::default()
        }
    );
    assert!(!f.exists("refresh_sessions", free_session).await);

    let next = clean_expired_auth_state(&f.pool, now, CleanupBatchSize::default())
        .await
        .unwrap();
    assert_eq!(
        next,
        CleanupCounts {
            refresh_sessions_deleted: 1,
            pending_registrations_deleted: 1,
            password_reset_hashes_cleared: 2,
            email_verification_hashes_cleared: 0,
        }
    );
    f.finish().await;
}

// ---------------------------------------------------------------------------
// Job order and report
// ---------------------------------------------------------------------------

#[tokio::test]
async fn job_commits_cleanup_before_mail_and_reports_counts() {
    let mut f = Fixture::new().await;
    let now = Utc::now();
    let user = f.user(Uuid::new_v4()).await;
    let session = f
        .session(user, Uuid::new_v4(), false, None, now - Duration::hours(1))
        .await;
    let transition = f.queue_mail(user, now - Duration::minutes(1)).await;

    // The provider call checks, from another process, that cleanup committed.
    let observer = f.second_pool().await;
    let sends = Arc::new(Mutex::new(Vec::new()));
    let recorded = sends.clone();
    let sender = move |payload: FrozenApprovalPayload, key: String| -> SendFuture {
        let observer = observer.clone();
        let recorded = recorded.clone();
        Box::pin(async move {
            let remaining: i64 =
                sqlx::query_scalar("SELECT count(*) FROM refresh_sessions WHERE id=$1")
                    .bind(session)
                    .fetch_one(&observer)
                    .await
                    .unwrap();
            recorded
                .lock()
                .unwrap()
                .push((payload.to[0].clone(), key, remaining));
            true
        })
    };
    let report = run(
        &f.pool,
        CleanupBatchSize::default(),
        ApprovalMailBatchSize::default(),
        Utc::now,
        sender,
    )
    .await
    .unwrap();
    assert_eq!(
        sends.lock().unwrap().clone(),
        vec![(recipient(user), key(transition), 0)],
        "one send, after the cleanup transaction committed"
    );
    assert_eq!(
        report.counts_json(),
        json!({
            "refresh_sessions_deleted": 1,
            "pending_registrations_deleted": 0,
            "password_reset_hashes_cleared": 0,
            "email_verification_hashes_cleared": 0,
            "approval_emails_attempted": 1,
            "approval_emails_completed": 1,
            "approval_emails_deferred": 0,
            "approval_emails_discarded": 0,
            "approval_emails_skipped": 0,
        })
    );
    assert_eq!(f.mail_state(user).await, (None, None, None));
    f.finish().await;
}

#[tokio::test]
async fn job_command_requires_apply_and_a_deployed_context() {
    let identity = IdentityArgs {
        environment: Environment::Staging,
        release_sha: "0123456789abcdef0123456789abcdef01234567".into(),
    };
    let dry = JobArgs {
        kind: JobKind::AuthMaintenance,
        identity: identity.clone(),
        apply: false,
    };
    assert_eq!(execute(&dry).await, Exit::Usage);
    // With --apply the purpose-bound context is still unavailable here, so
    // the command refuses before it opens a pool.
    let apply = JobArgs {
        kind: JobKind::AuthMaintenance,
        identity,
        apply: true,
    };
    assert_eq!(execute(&apply).await, Exit::Precondition);
    assert_eq!(
        AuthMaintenanceReport::default().counts_json()["approval_emails_attempted"],
        0
    );
}

// ---------------------------------------------------------------------------
// Approval mail
// ---------------------------------------------------------------------------

/// Two interleavings of cleanup and an approval claim on the same user:
/// (1) another sender holds the claim transaction open while the job runs;
/// (2) one job is inside its provider call while a second job runs.
/// Each time exactly one provider call happens for the transition.
#[tokio::test]
async fn cleanup_and_approval_claim_do_not_double_send() {
    let mut f = Fixture::new().await;
    let now = Utc::now();

    // (1) The claim holds the user row; the job's cleanup skips that row and
    // its approval step finds the transition no longer due.
    let first = f.user(Uuid::new_v4()).await;
    f.set_reset(first, &token(1), Some(now - Duration::hours(1)))
        .await;
    let first_transition = f.queue_mail(first, now - Duration::minutes(1)).await;
    let mut claimer = f.connection().await;
    let claimer_pid = backend_pid(&mut claimer).await;
    sqlx::query("BEGIN READ WRITE")
        .execute(&mut claimer)
        .await
        .unwrap();
    let claim = claim_approval_mail(&mut claimer, first, first_transition, Utc::now())
        .await
        .unwrap()
        .expect("the other sender claims the due transition");
    let job_provider = Provider::default();
    let pool = f.pool.clone();
    let sender = job_provider.sender(true);
    let job = tokio::spawn(async move {
        run(
            &pool,
            CleanupBatchSize::default(),
            ApprovalMailBatchSize::default(),
            Utc::now,
            sender,
        )
        .await
    });
    let mut watcher = f.connection().await;
    blocked_or_finished(&mut watcher, claimer_pid, &job).await;
    sqlx::query("COMMIT").execute(&mut claimer).await.unwrap();
    let other_provider = Provider::default();
    let accepted =
        (other_provider.sender(true))(claim.marker.payload.clone(), key(first_transition)).await;
    sqlx::query("BEGIN READ WRITE")
        .execute(&mut claimer)
        .await
        .unwrap();
    finalize_approval_mail(&mut claimer, &claim, accepted, Utc::now())
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut claimer).await.unwrap();
    let report = job.await.unwrap().unwrap();
    assert_eq!(
        report.approval.attempted, 1,
        "the job saw the due transition"
    );
    assert_eq!(report.approval.skipped, 1, "the claim made it not due");
    assert_eq!(report.approval.completed, 0);
    assert_eq!(
        report.cleanup.password_reset_hashes_cleared, 0,
        "the claimed row was skipped"
    );
    assert!(job_provider.sent().is_empty());
    assert_eq!(other_provider.sent().len(), 1);
    assert_eq!(f.mail_state(first).await, (None, None, None));

    // (2) Job A is inside its provider call. It holds no row lock, so job B
    // cleans the same user, and B sees no due transition.
    let second = f.user(Uuid::new_v4()).await;
    f.set_reset(second, &token(2), Some(now - Duration::hours(1)))
        .await;
    let second_transition = f.queue_mail(second, now - Duration::minutes(1)).await;
    let provider_a = Provider::default();
    let entered_a = Arc::new(Notify::new());
    let release_a = Arc::new(Notify::new());
    let pool_a = f.second_pool().await;
    let sender_a = provider_a.gated(entered_a.clone(), release_a.clone());
    let job_a = tokio::spawn(async move {
        retry_due(
            &pool_a,
            ApprovalMailBatchSize::default(),
            Utc::now,
            sender_a,
        )
        .await
    });
    entered(&entered_a).await;
    let provider_b = Provider::default();
    let report_b = run(
        &f.pool,
        CleanupBatchSize::default(),
        ApprovalMailBatchSize::default(),
        Utc::now,
        provider_b.sender(true),
    )
    .await
    .unwrap();
    release_a.notify_one();
    let counts_a = job_a.await.unwrap().unwrap();
    // B clears this user's expired digest during A's provider call, and the
    // first user's digest that the claim made the first job skip.
    assert_eq!(report_b.cleanup.password_reset_hashes_cleared, 2);
    assert_eq!(f.tokens(second).await.0, None);
    assert_eq!(f.tokens(first).await.0, None);
    assert_eq!(
        report_b.approval.attempted, 0,
        "the claimed transition is not due"
    );
    assert!(provider_b.sent().is_empty());
    assert_eq!(counts_a.completed, 1);
    assert_eq!(
        provider_a.sent(),
        vec![(recipient(second), key(second_transition))]
    );
    assert_eq!(f.mail_state(second).await, (None, None, None));
    f.finish().await;
}

#[tokio::test]
async fn approval_mail_claim_is_transition_scoped() {
    let mut f = Fixture::new().await;
    let now = Utc::now();

    // A re-approval replaces the transition during the provider call: the old
    // claim finalizes nothing and the new transition stays queued.
    let user = f.user(Uuid::new_v4()).await;
    let old_transition = f.queue_mail(user, now - Duration::minutes(1)).await;
    let provider = Provider::default();
    let entered_signal = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let pool = f.pool.clone();
    let sender = provider.gated(entered_signal.clone(), release.clone());
    let job = tokio::spawn(async move {
        retry_due(&pool, ApprovalMailBatchSize::default(), Utc::now, sender).await
    });
    entered(&entered_signal).await;
    let new_transition = Uuid::new_v4();
    let new_retry = now - Duration::seconds(1);
    sqlx::query(
        "UPDATE users SET pending_approval_email_transition_id=$2,\
         pending_approval_email_payload=$3,pending_approval_email_retry_at=$4 WHERE id=$1",
    )
    .bind(user)
    .bind(new_transition)
    .bind(payload("renewed@example.test"))
    .bind(new_retry)
    .execute(&mut f.admin)
    .await
    .unwrap();
    release.notify_one();
    let counts = job.await.unwrap().unwrap();
    assert_eq!(counts.attempted, 1);
    assert_eq!(counts.completed, 0);
    assert_eq!(counts.skipped, 1);
    assert_eq!(
        provider.sent(),
        vec![(recipient(user), key(old_transition))]
    );
    let (transition, stored, retry_at) = f.mail_state(user).await;
    assert_eq!(transition, Some(new_transition));
    assert_eq!(stored, Some(payload("renewed@example.test")));
    assert_eq!(
        retry_at.map(|t| t.timestamp_micros()),
        Some(new_retry.timestamp_micros())
    );

    // The next run delivers the new transition under its own key.
    let next = Provider::default();
    let counts = retry_due(
        &f.pool,
        ApprovalMailBatchSize::default(),
        Utc::now,
        next.sender(true),
    )
    .await
    .unwrap();
    assert_eq!(counts.completed, 1);
    assert_eq!(
        next.sent(),
        vec![("renewed@example.test".to_owned(), key(new_transition))]
    );
    assert_eq!(f.mail_state(user).await, (None, None, None));

    // A claim names one transition; another transition id claims nothing.
    let other = f.user(Uuid::new_v4()).await;
    let queued = f.queue_mail(other, now - Duration::minutes(1)).await;
    let before = f.mail_state(other).await;
    let stray = Provider::default();
    let outcome = deliver_due(
        &f.pool,
        other,
        Uuid::new_v4(),
        &Utc::now,
        &stray.sender(true),
    )
    .await
    .unwrap();
    assert_eq!(outcome, ApprovalOutcome::Skipped);
    assert!(stray.sent().is_empty());
    assert_eq!(f.mail_state(other).await, before);
    let delivered = deliver_due(&f.pool, other, queued, &Utc::now, &stray.sender(true))
        .await
        .unwrap();
    assert_eq!(delivered, ApprovalOutcome::Sent);
    assert_eq!(stray.sent(), vec![(recipient(other), key(queued))]);

    // An ineligible account or an unreadable payload is discarded unsent.
    let mut discarded = Vec::new();
    for change in [
        "status='PENDING'",
        "role='ADMIN'",
        "email_verified=false",
        "pending_approval_email_payload='[]'::jsonb",
    ] {
        let id = f.user(Uuid::new_v4()).await;
        f.queue_mail(id, now - Duration::minutes(1)).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE users SET {change} WHERE id=$1"
        )))
        .bind(id)
        .execute(&mut f.admin)
        .await
        .unwrap();
        discarded.push(id);
    }
    let none = Provider::default();
    let counts = retry_due(
        &f.pool,
        ApprovalMailBatchSize::default(),
        Utc::now,
        none.sender(true),
    )
    .await
    .unwrap();
    assert_eq!(counts.attempted, 4);
    assert_eq!(counts.discarded, 4);
    assert!(none.sent().is_empty());
    for id in discarded {
        assert_eq!(f.mail_state(id).await, (None, None, None));
    }
    f.finish().await;
}

#[tokio::test]
async fn approval_mail_failure_defers_one_hour() {
    let mut f = Fixture::new().await;
    let t0 = instant();
    let user = f.user(Uuid::new_v4()).await;
    let transition = f.queue_mail(user, t0 - Duration::minutes(1)).await;

    let refusing = Provider::default();
    let counts = retry_due(&f.pool, mail_batch(20), move || t0, refusing.sender(false))
        .await
        .unwrap();
    assert_eq!(counts.attempted, 1);
    assert_eq!(counts.deferred, 1);
    assert_eq!(counts.completed, 0);
    assert_eq!(refusing.sent(), vec![(recipient(user), key(transition))]);
    let (kept, stored, retry_at) = f.mail_state(user).await;
    assert_eq!(kept, Some(transition), "the transition is kept for retry");
    assert_eq!(
        stored,
        Some(payload(&recipient(user))),
        "the frozen payload is kept"
    );
    assert_eq!(retry_at, Some(t0 + Duration::hours(1)));

    let early = Provider::default();
    let before_due = t0 + Duration::minutes(59);
    let counts = retry_due(
        &f.pool,
        mail_batch(20),
        move || before_due,
        early.sender(true),
    )
    .await
    .unwrap();
    assert_eq!(counts.attempted, 0);
    assert!(early.sent().is_empty());

    let due = Provider::default();
    let at_due = t0 + Duration::hours(1);
    let counts = retry_due(&f.pool, mail_batch(20), move || at_due, due.sender(true))
        .await
        .unwrap();
    assert_eq!(counts.completed, 1);
    assert_eq!(due.sent(), vec![(recipient(user), key(transition))]);
    assert_eq!(f.mail_state(user).await, (None, None, None));
    f.finish().await;
}

#[tokio::test]
async fn approval_mail_queue_is_ordered_due_and_bounded() {
    let mut f = Fixture::new().await;
    let t0 = instant();
    let a = f.user(Uuid::from_u128(0x50)).await;
    let b = f.user(Uuid::from_u128(0x40)).await;
    let tie_low = f.user(Uuid::from_u128(0x10)).await;
    let tie_high = f.user(Uuid::from_u128(0x20)).await;
    let e = f.user(Uuid::from_u128(0x30)).await;
    let future = f.user(Uuid::from_u128(0x05)).await;
    f.queue_mail(a, t0 - Duration::minutes(5)).await;
    f.queue_mail(b, t0 - Duration::minutes(3)).await;
    f.queue_mail(tie_low, t0 - Duration::minutes(4)).await;
    f.queue_mail(tie_high, t0 - Duration::minutes(4)).await;
    f.queue_mail(e, t0 - Duration::minutes(1)).await;
    f.queue_mail(future, t0 + Duration::minutes(1)).await;

    let first = Provider::default();
    let counts = retry_due(&f.pool, mail_batch(3), move || t0, first.sender(true))
        .await
        .unwrap();
    assert_eq!(counts.attempted, 3);
    assert_eq!(counts.completed, 3);
    let order: Vec<String> = first.sent().into_iter().map(|(to, _)| to).collect();
    assert_eq!(
        order,
        vec![recipient(a), recipient(tie_low), recipient(tie_high)]
    );
    assert!(f.mail_state(b).await.0.is_some());
    assert!(f.mail_state(e).await.0.is_some());

    let second = Provider::default();
    let counts = retry_due(&f.pool, mail_batch(3), move || t0, second.sender(true))
        .await
        .unwrap();
    assert_eq!(counts.attempted, 2, "the future row is not due");
    let order: Vec<String> = second.sent().into_iter().map(|(to, _)| to).collect();
    assert_eq!(order, vec![recipient(b), recipient(e)]);
    assert!(f.mail_state(future).await.0.is_some());
    f.finish().await;
}

#[tokio::test]
async fn approval_mail_never_holds_a_user_lock_across_provider_io() {
    let mut f = Fixture::new().await;
    let user = f.user(Uuid::new_v4()).await;
    let transition = f.queue_mail(user, Utc::now() - Duration::minutes(1)).await;
    let provider = Provider::default();
    let entered_signal = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let pool = f.pool.clone();
    let sender = provider.gated(entered_signal.clone(), release.clone());
    let job = tokio::spawn(async move {
        retry_due(&pool, ApprovalMailBatchSize::default(), Utc::now, sender).await
    });
    entered(&entered_signal).await;

    // While the provider call is open, another request locks and writes the row
    // at once, and the job has no open transaction.
    let mut probe = f.connection().await;
    sqlx::query("BEGIN").execute(&mut probe).await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(user)
        .execute(&mut probe)
        .await
        .expect("no lock is held on the user row during provider I/O");
    sqlx::query("UPDATE users SET last_login=now() WHERE id=$1")
        .bind(user)
        .execute(&mut probe)
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut probe).await.unwrap();
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() \
         AND usename='rust_app' AND state LIKE 'idle in transaction%'",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(open, 0, "no transaction stays open across provider I/O");

    release.notify_one();
    let counts = job.await.unwrap().unwrap();
    assert_eq!(counts.completed, 1);
    assert_eq!(provider.sent(), vec![(recipient(user), key(transition))]);
    assert_eq!(f.mail_state(user).await, (None, None, None));
    f.finish().await;
}

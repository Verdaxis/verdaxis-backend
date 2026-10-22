//! Auth storage and lock primitives.
//!
//! The offline tests always run. The PostgreSQL tests need a database at the
//! Alembic head with the runtime ACLs applied; the Python harness
//! `tests/postgres/test_phase3_auth_store.py` provides one and sets
//! `AUTH_STORE_TEST_DATABASE_URL` (the runtime role, which runs the code under
//! test) and `AUTH_STORE_TEST_OWNER_URL` (the table owner, which seeds and
//! removes fixture rows). Each test uses its own ids, so tests do not share rows.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use tokio::sync::Barrier;
use uuid::Uuid;
use verdaxis_read::auth::{
    lock::{
        ORG_WRITE_ATTEMPTS, OrgWriteError, OrganizationWriteScope, WriteConflict, device_lock_key,
        lock_organization_write, retry_org_write,
    },
    store::{
        NewRefreshRow, NewResetToken, consume_reset_token, insert_refresh, lock_device,
        lock_refresh_by_jti, lock_user, lock_user_by_email, reset_token_owner,
        revoke_device_families, revoke_user_families, sweep_expired_reset_tokens,
        upsert_reset_token,
    },
};

const DEVICE_LOCK_VECTORS: &str =
    include_str!("../../../tests/rust_contract/phase3/device-lock-keys.json");

/// Observe the database wait, rather than treating a slow connection as a lock.
async fn wait_until_blocked(observer: &mut PgConnection, pid: i32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                .bind(pid)
                .fetch_one(&mut *observer)
                .await
                .unwrap();
            if blocked {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("writer must reach a database lock wait");
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn device_lock_key_matches_python() {
    let vectors: Value = serde_json::from_str(DEVICE_LOCK_VECTORS).unwrap();
    let cookies = vectors["cookies"].as_array().unwrap();
    assert_eq!(cookies.len(), 20);
    for vector in cookies {
        let digest = sha256_hex(vector["cookie"].as_str().unwrap());
        assert_eq!(digest, vector["digest"].as_str().unwrap());
        assert_eq!(device_lock_key(&digest), vector["key"].as_i64());
    }
    let negative = cookies
        .iter()
        .filter(|vector| vector["key"].as_i64().unwrap() < 0)
        .count();
    assert!(negative > 0 && negative < cookies.len());
    for vector in vectors["digests"].as_array().unwrap() {
        let digest = vector["digest"].as_str().unwrap();
        assert_eq!(device_lock_key(digest), vector["key"].as_i64(), "{digest}");
    }
}

#[test]
fn device_lock_key_refuses_anything_but_a_lowercase_digest() {
    let digest = sha256_hex("p3-device-lock-shape");
    assert!(device_lock_key(&digest).is_some());
    for refused in [
        String::new(),
        digest[..63].to_owned(),
        format!("{digest}0"),
        digest.to_uppercase(),
        format!("g{}", &digest[1..]),
        format!("-{}", &digest[1..]),
    ] {
        assert_eq!(device_lock_key(&refused), None, "{refused}");
    }
}

/// A scripted attempt failure for the retry tests.
#[derive(Debug, PartialEq, Eq)]
enum Scripted {
    Conflict,
    Refused,
}

impl WriteConflict for Scripted {
    fn is_write_conflict(&self) -> bool {
        *self == Self::Conflict
    }
}

/// Run `retry_org_write` with one scripted outcome per attempt and report the
/// result and the number of attempts made.
async fn run_scripted(
    outcomes: &[Result<&'static str, Scripted>],
) -> (Result<&'static str, OrgWriteError<Scripted>>, usize) {
    let calls = AtomicUsize::new(0);
    let result = retry_org_write(|attempt| {
        let index = calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(attempt, index + 1);
        let outcome = match &outcomes[index] {
            Ok(value) => Ok(*value),
            Err(Scripted::Conflict) => Err(Scripted::Conflict),
            Err(Scripted::Refused) => Err(Scripted::Refused),
        };
        async move { outcome }
    })
    .await;
    (result, calls.load(Ordering::SeqCst))
}

#[tokio::test]
async fn org_write_retries_twice_then_stops() {
    assert_eq!(ORG_WRITE_ATTEMPTS, 3);

    let (result, attempts) = run_scripted(&[Ok("committed")]).await;
    assert_eq!((result.unwrap(), attempts), ("committed", 1));

    let (result, attempts) = run_scripted(&[Err(Scripted::Conflict), Ok("committed")]).await;
    assert_eq!((result.unwrap(), attempts), ("committed", 2));

    let twice = [
        Err(Scripted::Conflict),
        Err(Scripted::Conflict),
        Ok("committed"),
    ];
    let (result, attempts) = run_scripted(&twice).await;
    assert_eq!((result.unwrap(), attempts), ("committed", 3));

    let always = [
        Err(Scripted::Conflict),
        Err(Scripted::Conflict),
        Err(Scripted::Conflict),
        Ok("never reached"),
    ];
    let (result, attempts) = run_scripted(&always).await;
    assert!(matches!(result, Err(OrgWriteError::Busy)));
    assert_eq!(attempts, 3);

    let refused = [Err(Scripted::Conflict), Err(Scripted::Refused), Ok("never")];
    let (result, attempts) = run_scripted(&refused).await;
    assert!(matches!(
        result,
        Err(OrgWriteError::Failed(Scripted::Refused))
    ));
    assert_eq!(attempts, 2);

    let busy = verdaxis_read::ApiError::from(
        OrgWriteError::<verdaxis_read::auth::lock::OrgWriteAttemptError>::Busy,
    );
    assert_eq!(busy.status, 503);
    assert_eq!(busy.retry_after, Some(1));
    assert_eq!(busy.detail["code"], "ORGANIZATION_WRITE_BUSY");
    use verdaxis_read::auth::lock::OrgWriteAttemptError;
    let result = retry_org_write(|_| async {
        Err::<(), _>(OrgWriteAttemptError::from(
            verdaxis_read::ApiError::database(),
        ))
    })
    .await;
    assert!(matches!(
        result,
        Err(OrgWriteError::Failed(OrgWriteAttemptError::Application(_)))
    ));
    let answer = verdaxis_read::ApiError::from(result.unwrap_err());
    assert_eq!(answer.status, 500);
}

#[test]
fn organization_write_scope_is_sorted_and_unique() {
    let [a, b, c] = [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)];
    let scope = OrganizationWriteScope::new([c, a, c], [b, a, b], [c, b]);
    assert_eq!(scope.organizations(), [a, c]);
    assert_eq!(scope.users(), [a, b]);
    assert_eq!(scope.join_requests(), [b, c]);
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

struct Database {
    runtime: String,
    owner: String,
}

/// Explicitly ignored database tests must fail if invoked without their URLs.
fn database() -> Database {
    let runtime = std::env::var("AUTH_STORE_TEST_DATABASE_URL").expect("runtime URL");
    let owner = std::env::var("AUTH_STORE_TEST_OWNER_URL").expect("owner URL");
    Database { runtime, owner }
}

async fn connect(url: &str) -> PgConnection {
    PgConnection::connect(url).await.unwrap()
}

async fn begin(connection: &mut PgConnection) {
    sqlx::query("BEGIN")
        .execute(&mut *connection)
        .await
        .unwrap();
}

async fn commit(connection: &mut PgConnection) {
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .unwrap();
}

async fn rollback(connection: &mut PgConnection) {
    sqlx::query("ROLLBACK")
        .execute(&mut *connection)
        .await
        .unwrap();
}

/// Rows one test created through the owner connection, removed at the end.
#[derive(Default)]
struct Fixture {
    organizations: Vec<Uuid>,
    users: Vec<Uuid>,
    join_requests: Vec<Uuid>,
}

impl Fixture {
    async fn organization(&mut self, owner: &mut PgConnection) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations (id, name, type) VALUES ($1, $2, 'FUEL_BUYER')")
            .bind(id)
            .bind(format!("Auth store {id}"))
            .execute(&mut *owner)
            .await
            .unwrap();
        self.organizations.push(id);
        id
    }

    async fn user(&mut self, owner: &mut PgConnection, organization: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, status, organization_id, \
             email_verified, must_change_password) \
             VALUES ($1, $2, 'not-a-login', 'BUYER', 'APPROVED', $3, true, false)",
        )
        .bind(id)
        .bind(format!("auth-store-{id}@phase3.invalid"))
        .bind(organization)
        .execute(&mut *owner)
        .await
        .unwrap();
        self.users.push(id);
        id
    }

    async fn join_request(&mut self, owner: &mut PgConnection, user: Uuid, org: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organization_join_requests (id, user_id, organization_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(id)
        .bind(user)
        .bind(org)
        .execute(&mut *owner)
        .await
        .unwrap();
        self.join_requests.push(id);
        id
    }

    async fn remove(self, owner: &mut PgConnection) {
        for (statement, ids) in [
            (
                "DELETE FROM organization_join_requests WHERE id = ANY($1)",
                &self.join_requests,
            ),
            (
                "DELETE FROM refresh_sessions WHERE user_id = ANY($1)",
                &self.users,
            ),
            ("DELETE FROM users WHERE id = ANY($1)", &self.users),
            (
                "DELETE FROM organizations WHERE id = ANY($1)",
                &self.organizations,
            ),
        ] {
            sqlx::query(statement)
                .bind(ids.as_slice())
                .execute(&mut *owner)
                .await
                .unwrap();
        }
    }
}

fn new_refresh(user_id: Uuid, family_id: Uuid, device_hash: &str) -> NewRefreshRow {
    NewRefreshRow {
        user_id,
        family_id,
        jti_hash: sha256_hex(&Uuid::new_v4().to_string()),
        device_id_hash: device_hash.to_owned(),
        expires_at: Utc::now() + TimeDelta::days(7),
    }
}

/// Wait until `task` has finished, or report that it is still blocked.
async fn finished_within<T>(task: &mut tokio::task::JoinHandle<T>, wait: Duration) -> Option<T> {
    match tokio::time::timeout(wait, task).await {
        Ok(joined) => Some(joined.unwrap()),
        Err(_) => None,
    }
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn auth_lock_order_serializes_device_user_refresh() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let user = fixture.user(&mut owner, None).await;
    let device_hash = sha256_hex(&format!("auth-store-device-{user}"));
    let refresh = new_refresh(user, Uuid::new_v4(), &device_hash);
    let mut seed = connect(&database.runtime).await;
    insert_refresh(&mut seed, &refresh).await.unwrap();

    // The first session writer takes all three locks in the canonical order.
    let mut first = connect(&database.runtime).await;
    begin(&mut first).await;
    lock_device(&mut first, &device_hash).await.unwrap();
    let locked = lock_user(&mut first, user).await.unwrap().unwrap();
    assert_eq!(locked.id, user);
    let row = lock_refresh_by_jti(&mut first, &refresh.jti_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.user_id, row.family_id), (user, refresh.family_id));

    // Python calls pg_advisory_xact_lock with this key for the same device.
    let key = device_lock_key(&device_hash).unwrap();
    let mut probe = connect(&database.runtime).await;
    begin(&mut probe).await;
    sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut probe)
        .await
        .unwrap();
    let waited = sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut probe)
        .await
        .unwrap_err();
    let code = waited.as_database_error().and_then(|error| error.code());
    assert_eq!(
        code.as_deref(),
        Some("55P03"),
        "the device lock uses the shared key"
    );
    rollback(&mut probe).await;

    // A second writer in the same order waits at the device lock and then
    // completes once the first commits: no deadlock and no error.
    let mut connection = connect(&database.runtime).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let (second_device, second_jti) = (device_hash.clone(), refresh.jti_hash.clone());
    let mut second = tokio::spawn(async move {
        begin(&mut connection).await;
        lock_device(&mut connection, &second_device).await?;
        let user = lock_user(&mut connection, user).await?;
        let row = lock_refresh_by_jti(&mut connection, &second_jti).await?;
        commit(&mut connection).await;
        Ok::<_, sqlx::Error>((user.is_some(), row.is_some()))
    });
    wait_until_blocked(&mut owner, pid).await;
    commit(&mut first).await;
    let completed = finished_within(&mut second, Duration::from_secs(10)).await;
    assert_eq!(
        completed.expect("second writer completes").unwrap(),
        (true, true)
    );

    fixture.remove(&mut owner).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn user_rows_lock_by_id_and_by_normalized_email() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let organization = fixture.organization(&mut owner).await;
    let user = fixture.user(&mut owner, Some(organization)).await;

    let mut connection = connect(&database.runtime).await;
    begin(&mut connection).await;
    let by_id = lock_user(&mut connection, user).await.unwrap().unwrap();
    assert_eq!(by_id.email, format!("auth-store-{user}@phase3.invalid"));
    assert_eq!(
        (by_id.status.as_str(), by_id.role.as_deref()),
        ("APPROVED", Some("BUYER"))
    );
    assert_eq!(by_id.organization_id, Some(organization));
    assert!(by_id.email_verified && !by_id.must_change_password);
    let debug = format!("{by_id:?}");
    assert!(!debug.contains("not-a-login") && !debug.contains("phase3.invalid"));

    let by_email = lock_user_by_email(&mut connection, &by_id.email)
        .await
        .unwrap();
    assert_eq!(by_email.map(|row| row.id), Some(user));
    assert!(
        lock_user(&mut connection, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
    let missing = lock_user_by_email(&mut connection, "missing@phase3.invalid").await;
    assert!(missing.unwrap().is_none());
    rollback(&mut connection).await;

    fixture.remove(&mut owner).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn refresh_rows_insert_lock_and_revoke() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let user = fixture.user(&mut owner, None).await;
    let other = fixture.user(&mut owner, None).await;
    let device_a = sha256_hex(&format!("auth-store-a-{user}"));
    let device_b = sha256_hex(&format!("auth-store-b-{user}"));
    let on_a = new_refresh(user, Uuid::new_v4(), &device_a);
    let on_b = new_refresh(user, Uuid::new_v4(), &device_b);
    let other_on_a = new_refresh(other, Uuid::new_v4(), &device_a);
    // A family from before device binding: Python stores no device hash.
    let legacy_family = Uuid::new_v4();
    let legacy_jti = sha256_hex(&format!("auth-store-legacy-{user}"));
    sqlx::query(
        "INSERT INTO refresh_sessions (id, user_id, family_id, jti_hash, expires_at) \
         VALUES ($1, $2, $3, $4, now() + interval '7 days')",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(legacy_family)
    .bind(&legacy_jti)
    .execute(&mut owner)
    .await
    .unwrap();

    let mut connection = connect(&database.runtime).await;
    begin(&mut connection).await;
    for row in [&on_a, &on_b, &other_on_a] {
        insert_refresh(&mut connection, row).await.unwrap();
    }
    let stored = lock_refresh_by_jti(&mut connection, &on_a.jti_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            stored.user_id,
            stored.family_id,
            stored.device_id_hash.as_deref()
        ),
        (user, on_a.family_id, Some(device_a.as_str()))
    );
    assert!(!stored.revoked && stored.replaced_by_jti_hash.is_none());
    assert_eq!(stored.rotation_grace_until, None);
    assert_eq!(
        stored.expires_at.timestamp_micros(),
        on_a.expires_at.timestamp_micros()
    );
    assert!(!format!("{stored:?}").contains(&on_a.jti_hash));
    let unknown = sha256_hex("auth-store-unknown");
    assert!(
        lock_refresh_by_jti(&mut connection, &unknown)
            .await
            .unwrap()
            .is_none()
    );

    // Device A's families of both users, plus the presented legacy family.
    let revoked = revoke_device_families(&mut connection, &device_a, Some(legacy_family))
        .await
        .unwrap();
    assert_eq!(revoked, 3);
    let again = revoke_device_families(&mut connection, &device_a, None)
        .await
        .unwrap();
    assert_eq!(again, 0);
    let on_b_row = lock_refresh_by_jti(&mut connection, &on_b.jti_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(!on_b_row.revoked);

    assert_eq!(
        revoke_user_families(&mut connection, user).await.unwrap(),
        1
    );
    assert_eq!(
        revoke_user_families(&mut connection, user).await.unwrap(),
        0
    );
    commit(&mut connection).await;

    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refresh_sessions WHERE user_id = ANY($1) AND NOT revoked",
    )
    .bind(vec![user, other])
    .fetch_one(&mut owner)
    .await
    .unwrap();
    assert_eq!(live, 0);

    fixture.remove(&mut owner).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn reset_tokens_replace_and_consume_once() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let user = fixture.user(&mut owner, None).await;
    let expired_user = fixture.user(&mut owner, None).await;
    let now = Utc::now();
    let first = sha256_hex(&format!("reset-first-{user}"));
    let second = sha256_hex(&format!("reset-second-{user}"));
    let expired = sha256_hex(&format!("reset-expired-{user}"));
    let token = |user_id: Uuid, token_hash: &str, expires_at: DateTime<Utc>| NewResetToken {
        user_id,
        token_hash: token_hash.to_owned(),
        expires_at,
    };

    let mut connection = connect(&database.runtime).await;
    for raw in ["raw-reset-token", "A".repeat(64).as_str()] {
        assert!(matches!(
            reset_token_owner(&mut connection, raw).await,
            Err(sqlx::Error::InvalidArgument(_))
        ));
        assert!(matches!(
            consume_reset_token(&mut connection, user, raw, now).await,
            Err(sqlx::Error::InvalidArgument(_))
        ));
        assert!(matches!(
            upsert_reset_token(&mut connection, &token(user, raw, now), now).await,
            Err(sqlx::Error::InvalidArgument(_))
        ));
    }
    begin(&mut connection).await;
    lock_user(&mut connection, expired_user).await.unwrap();
    let past = now - TimeDelta::hours(2);
    upsert_reset_token(&mut connection, &token(expired_user, &expired, now), past)
        .await
        .unwrap();
    lock_user(&mut connection, user).await.unwrap();
    let hour = now + TimeDelta::hours(1);
    upsert_reset_token(&mut connection, &token(user, &first, hour), now)
        .await
        .unwrap();
    // Issuing a token never changes another user's expired row.
    assert_eq!(
        reset_token_owner(&mut connection, &expired).await.unwrap(),
        Some(expired_user)
    );
    assert_eq!(
        reset_token_owner(&mut connection, &first).await.unwrap(),
        Some(user)
    );

    // A second request replaces the first token.
    upsert_reset_token(&mut connection, &token(user, &second, hour), now)
        .await
        .unwrap();
    assert_eq!(
        reset_token_owner(&mut connection, &first).await.unwrap(),
        None
    );
    assert!(
        !consume_reset_token(&mut connection, user, &first, now)
            .await
            .unwrap()
    );
    let other = consume_reset_token(&mut connection, expired_user, &second, now).await;
    assert!(!other.unwrap(), "a token is consumed only for its owner");
    let late = hour + TimeDelta::seconds(1);
    assert!(
        !consume_reset_token(&mut connection, user, &second, late)
            .await
            .unwrap()
    );
    assert!(
        consume_reset_token(&mut connection, user, &second, now)
            .await
            .unwrap()
    );
    assert!(
        !consume_reset_token(&mut connection, user, &second, now)
            .await
            .unwrap()
    );
    rollback(&mut connection).await;

    fixture.remove(&mut owner).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn expired_reset_cleanup_skips_locked_rows() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let users = [
        fixture.user(&mut owner, None).await,
        fixture.user(&mut owner, None).await,
        fixture.user(&mut owner, None).await,
    ];
    let now = Utc::now();
    for (index, user) in users.iter().enumerate() {
        sqlx::query("INSERT INTO password_reset_tokens (user_id, token_hash, expires_at) VALUES ($1, $2, $3)")
            .bind(user)
            .bind(sha256_hex(&format!("cleanup-{user}")))
            .bind(if index == 2 { now + TimeDelta::hours(1) } else { now - TimeDelta::hours(1) })
            .execute(&mut owner).await.unwrap();
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database.runtime)
        .await
        .unwrap();
    let mut holder = connect(&database.runtime).await;
    begin(&mut holder).await;
    sqlx::query("SELECT user_id FROM password_reset_tokens WHERE user_id = $1 FOR UPDATE")
        .bind(users[0])
        .execute(&mut holder)
        .await
        .unwrap();
    sweep_expired_reset_tokens(&pool, now).await.unwrap();
    let retained: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM password_reset_tokens WHERE user_id = ANY($1)")
            .bind(users.to_vec())
            .fetch_all(&mut owner)
            .await
            .unwrap();
    assert!(retained.contains(&users[0]) && retained.contains(&users[2]));
    assert!(!retained.contains(&users[1]));
    rollback(&mut holder).await;
    sweep_expired_reset_tokens(&pool, now).await.unwrap();
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM password_reset_tokens WHERE user_id = ANY($1)")
            .bind(users.to_vec())
            .fetch_all(&mut owner)
            .await
            .unwrap();
    assert_eq!(remaining, [users[2]]);
    pool.close().await;
    fixture.remove(&mut owner).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn reset_token_issuance_keeps_other_users_independent() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let users = [
        fixture.user(&mut owner, None).await,
        fixture.user(&mut owner, None).await,
    ];
    let now = Utc::now();
    for user in users {
        sqlx::query(
            "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at) VALUES ($1, $2, $3)",
        )
        .bind(user)
        .bind(sha256_hex(&format!("expired-{user}")))
        .bind(now - TimeDelta::hours(1))
        .execute(&mut owner)
        .await
        .unwrap();
    }

    let start = Arc::new(Barrier::new(2));
    let issued = Arc::new(Barrier::new(2));
    let mut tasks = Vec::new();
    for user in users {
        let (start, issued, runtime) = (start.clone(), issued.clone(), database.runtime.clone());
        tasks.push(tokio::spawn(async move {
            let mut connection = connect(&runtime).await;
            let mut transaction = connection.begin().await.unwrap();
            lock_user(&mut transaction, user).await.unwrap();
            start.wait().await;
            let token = NewResetToken {
                user_id: user,
                token_hash: sha256_hex(&format!("replacement-{user}")),
                expires_at: now + TimeDelta::hours(1),
            };
            // Both writes must finish before either transaction commits. A sweep
            // of another user's row makes one writer wait on the other writer,
            // which is waiting at the completion barrier: a dependency cycle.
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                upsert_reset_token(&mut transaction, &token, now)
                    .await
                    .unwrap();
                issued.wait().await;
            })
            .await;
            if result.is_ok() {
                transaction.commit().await.unwrap();
            } else {
                transaction.rollback().await.unwrap();
            }
            result
        }));
    }
    let mut completed = Vec::new();
    for task in tasks {
        completed.push(task.await.unwrap());
    }
    fixture.remove(&mut owner).await;
    assert!(
        completed.iter().all(Result::is_ok),
        "issuance must not wait on another user's reset row: {completed:?}"
    );
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn organization_write_locks_follow_one_order() {
    let database = database();
    let mut owner = connect(&database.owner).await;
    let mut fixture = Fixture::default();
    let org_a = fixture.organization(&mut owner).await;
    let org_b = fixture.organization(&mut owner).await;
    let user_a = fixture.user(&mut owner, Some(org_a)).await;
    let user_b = fixture.user(&mut owner, Some(org_b)).await;
    let join = fixture.join_request(&mut owner, user_a, org_b).await;
    let absent = Uuid::new_v4();

    // The same rows, named in opposite orders by two writers.
    let forward = OrganizationWriteScope::new([org_a, org_b], [user_a, user_b], [join]);
    let backward = OrganizationWriteScope::new([org_b, org_a, absent], [user_b, user_a], [join]);

    let mut first = connect(&database.runtime).await;
    begin(&mut first).await;
    let locked = lock_organization_write(&mut first, &forward).await.unwrap();
    assert_eq!(locked.organizations, forward.organizations());
    assert_eq!(locked.users, forward.users());
    assert_eq!(locked.join_requests, [join]);

    let barrier = Arc::new(Barrier::new(2));
    let waiting = barrier.clone();
    let mut connection = connect(&database.runtime).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let mut second = tokio::spawn(async move {
        begin(&mut connection).await;
        waiting.wait().await;
        let locked = lock_organization_write(&mut connection, &backward).await?;
        commit(&mut connection).await;
        Ok::<_, sqlx::Error>(locked)
    });
    barrier.wait().await;
    wait_until_blocked(&mut owner, pid).await;
    commit(&mut first).await;
    let completed = finished_within(&mut second, Duration::from_secs(10)).await;
    let locked = completed.expect("second writer completes").unwrap();
    let mut present = vec![org_a, org_b];
    present.sort_unstable();
    assert_eq!(
        locked.organizations, present,
        "an absent id is not reported"
    );

    fixture.remove(&mut owner).await;
}

/// A statement that fails with `sqlstate`. SQLx takes only static SQL text.
fn raise(sqlstate: &str) -> &'static str {
    match sqlstate {
        "40P01" => "DO $$ BEGIN RAISE EXCEPTION 'x' USING ERRCODE = '40P01'; END $$",
        "40001" => "DO $$ BEGIN RAISE EXCEPTION 'x' USING ERRCODE = '40001'; END $$",
        "55P03" => "DO $$ BEGIN RAISE EXCEPTION 'x' USING ERRCODE = '55P03'; END $$",
        other => panic!("no scripted statement for {other}"),
    }
}

/// Run one attempt that raises `sqlstate` in a fresh transaction, or succeeds
/// when `sqlstate` is `None`.
async fn raising_attempt(url: String, sqlstate: Option<&'static str>) -> Result<(), sqlx::Error> {
    let mut connection = connect(&url).await;
    begin(&mut connection).await;
    let raised = match sqlstate {
        Some(code) => sqlx::query(raise(code))
            .execute(&mut connection)
            .await
            .map(drop),
        None => Ok(()),
    };
    match raised {
        Ok(()) => commit(&mut connection).await,
        Err(_) => rollback(&mut connection).await,
    }
    raised
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn org_write_retry_classifies_postgres_conflicts() {
    let database = database();

    let calls = AtomicUsize::new(0);
    let schedule = [Some("40P01"), Some("40001"), None];
    let result = retry_org_write(|attempt| {
        calls.fetch_add(1, Ordering::SeqCst);
        raising_attempt(database.runtime.clone(), schedule[attempt - 1])
    })
    .await;
    assert!(result.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let calls = AtomicUsize::new(0);
    let result = retry_org_write(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        raising_attempt(database.runtime.clone(), Some("40001"))
    })
    .await;
    assert!(matches!(result, Err(OrgWriteError::Busy)));
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    // A lock that is not available is not a write conflict.
    let calls = AtomicUsize::new(0);
    let result = retry_org_write(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        raising_attempt(database.runtime.clone(), Some("55P03"))
    })
    .await;
    assert!(matches!(result, Err(OrgWriteError::Failed(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

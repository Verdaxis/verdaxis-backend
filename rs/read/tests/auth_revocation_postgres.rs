//! Run with the migrated disposable database and runtime/owner URLs described
//! in `auth_store_postgres.rs`. Plain offline runs report these tests as ignored.

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::{Connection, PgConnection};
use tokio::{sync::Barrier, task::JoinSet};
use uuid::Uuid;
use verdaxis_read::auth::store::{
    NewRefreshRow, insert_refresh, lock_user, revoke_user_authentication,
};

struct Fixture {
    owner: PgConnection,
    runtime_url: String,
    user: Uuid,
}

impl Fixture {
    async fn new(cutoff: Option<DateTime<Utc>>, sessions: usize) -> Self {
        let owner_url = std::env::var("AUTH_STORE_TEST_OWNER_URL").expect("owner URL");
        let runtime_url = std::env::var("AUTH_STORE_TEST_DATABASE_URL").expect("runtime URL");
        let mut owner = PgConnection::connect(&owner_url).await.unwrap();
        let user = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, status, \
             email_verified, must_change_password, authentication_revoked_at) \
             VALUES ($1, $2, 'not-a-login', 'BUYER', 'APPROVED', true, false, $3)",
        )
        .bind(user)
        .bind(format!("auth-revocation-{user}@phase3.invalid"))
        .bind(cutoff)
        .execute(&mut owner)
        .await
        .unwrap();
        let fixture = Self {
            owner,
            runtime_url,
            user,
        };
        let mut runtime = fixture.connect().await;
        for _ in 0..sessions {
            insert_refresh(
                &mut runtime,
                &NewRefreshRow {
                    user_id: user,
                    family_id: Uuid::new_v4(),
                    jti_hash: format!(
                        "{:032x}{:032x}",
                        Uuid::new_v4().as_u128(),
                        Uuid::new_v4().as_u128()
                    ),
                    device_id_hash: "a".repeat(64),
                    expires_at: instant() + TimeDelta::days(7),
                },
            )
            .await
            .unwrap();
        }
        fixture
    }

    async fn connect(&self) -> PgConnection {
        PgConnection::connect(&self.runtime_url).await.unwrap()
    }

    async fn remove(mut self) {
        sqlx::query("DELETE FROM refresh_sessions WHERE user_id = $1")
            .bind(self.user)
            .execute(&mut self.owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(self.user)
            .execute(&mut self.owner)
            .await
            .unwrap();
    }
}

fn instant() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-23T12:00:00.123456Z")
        .unwrap()
        .with_timezone(&Utc)
}

async fn cutoff(connection: &mut PgConnection, user: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT authentication_revoked_at FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(connection)
        .await
        .unwrap()
}

async fn sessions(connection: &mut PgConnection, user: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE NOT revoked) \
         FROM refresh_sessions WHERE user_id = $1",
    )
    .bind(user)
    .fetch_one(connection)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn revocation_cutoff_is_monotonic_under_concurrency() {
    const CALLS: usize = 20;
    let initial = instant();
    let mut fixture = Fixture::new(Some(initial), 4).await;
    let mut holder = fixture.connect().await;
    let mut held = holder.begin().await.unwrap();
    lock_user(&mut held, fixture.user).await.unwrap().unwrap();

    let barrier = Arc::new(Barrier::new(CALLS + 1));
    let mut tasks = JoinSet::new();
    let mut pids = Vec::new();
    for index in 0..CALLS {
        let mut connection = fixture.connect().await;
        pids.push(
            sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
                .fetch_one(&mut connection)
                .await
                .unwrap(),
        );
        let barrier = Arc::clone(&barrier);
        let user = fixture.user;
        let now = initial + TimeDelta::seconds(index as i64 - 10);
        tasks.spawn(async move {
            let mut transaction = connection.begin().await.unwrap();
            barrier.wait().await;
            let result = revoke_user_authentication(&mut transaction, user, now)
                .await
                .unwrap();
            assert_eq!(cutoff(&mut transaction, user).await, Some(result));
            assert_eq!(sessions(&mut transaction, user).await, (4, 0));
            transaction.commit().await.unwrap();
            (now, result)
        });
    }
    barrier.wait().await;
    // Hold the user until every call actually contends on a database lock.
    // Poll database state, not elapsed time, to establish the schedule.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let all_blocked: bool = sqlx::query_scalar(
                "SELECT bool_and(cardinality(pg_blocking_pids(pid)) > 0) \
                 FROM unnest($1::int[]) AS pid",
            )
            .bind(&pids)
            .fetch_one(&mut fixture.owner)
            .await
            .unwrap();
            if all_blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all revocation calls must wait for the user row");
    held.rollback().await.unwrap();

    let results = tokio::time::timeout(Duration::from_secs(20), async {
        let mut results = Vec::new();
        while let Some(result) = tasks.join_next().await {
            results.push(result.unwrap());
        }
        results
    })
    .await
    .expect("all revocation calls must complete");
    assert_eq!(results.len(), CALLS);
    let mut ordered = results;
    ordered.sort_unstable_by_key(|(_, result)| *result);
    let mut previous = initial;
    for (now, result) in &ordered {
        let expected = if *now > previous {
            *now
        } else {
            previous + TimeDelta::microseconds(1)
        };
        assert_eq!(*result, expected, "each call advances the preceding cutoff");
        previous = *result;
    }
    assert_eq!(cutoff(&mut fixture.owner, fixture.user).await, Some(previous));
    assert_eq!(sessions(&mut fixture.owner, fixture.user).await, (4, 0));
    fixture.remove().await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn equal_or_earlier_time_advances_cutoff_by_one_microsecond() {
    let initial = instant();
    let fixture = Fixture::new(Some(initial), 2).await;
    let mut connection = fixture.connect().await;
    let mut transaction = connection.begin().await.unwrap();
    for (index, now) in [initial, initial - TimeDelta::days(1)].into_iter().enumerate() {
        let expected = initial + TimeDelta::microseconds(index as i64 + 1);
        let result = revoke_user_authentication(&mut transaction, fixture.user, now)
            .await
            .unwrap();
        assert_eq!(result, expected);
        assert_eq!(cutoff(&mut transaction, fixture.user).await, Some(expected));
    }
    assert_eq!(sessions(&mut transaction, fixture.user).await, (2, 0));
    transaction.commit().await.unwrap();
    fixture.remove().await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn caller_rollback_preserves_cutoff_and_sessions() {
    let initial = instant();
    let mut fixture = Fixture::new(Some(initial), 3).await;
    let mut connection = fixture.connect().await;
    let mut transaction = connection.begin().await.unwrap();
    let now = initial + TimeDelta::seconds(1);
    assert_eq!(
        revoke_user_authentication(&mut transaction, fixture.user, now)
            .await
            .unwrap(),
        now
    );
    assert_eq!(cutoff(&mut transaction, fixture.user).await, Some(now));
    assert_eq!(sessions(&mut transaction, fixture.user).await, (3, 0));
    assert_eq!(cutoff(&mut fixture.owner, fixture.user).await, Some(initial));
    assert_eq!(sessions(&mut fixture.owner, fixture.user).await, (3, 3));
    transaction.rollback().await.unwrap();
    assert_eq!(cutoff(&mut fixture.owner, fixture.user).await, Some(initial));
    assert_eq!(sessions(&mut fixture.owner, fixture.user).await, (3, 3));
    fixture.remove().await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn user_without_refresh_sessions_gets_initial_cutoff() {
    let mut fixture = Fixture::new(None, 0).await;
    let mut connection = fixture.connect().await;
    let mut transaction = connection.begin().await.unwrap();
    let now = instant();
    assert_eq!(
        revoke_user_authentication(&mut transaction, fixture.user, now)
            .await
            .unwrap(),
        now
    );
    transaction.commit().await.unwrap();
    assert_eq!(cutoff(&mut fixture.owner, fixture.user).await, Some(now));
    assert_eq!(sessions(&mut fixture.owner, fixture.user).await, (0, 0));
    fixture.remove().await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn unknown_user_returns_row_not_found() {
    let mut fixture = Fixture::new(Some(instant()), 1).await;
    let mut connection = fixture.connect().await;
    let mut transaction = connection.begin().await.unwrap();
    assert!(matches!(
        revoke_user_authentication(&mut transaction, Uuid::new_v4(), instant()).await,
        Err(sqlx::Error::RowNotFound)
    ));
    transaction.rollback().await.unwrap();
    assert_eq!(cutoff(&mut fixture.owner, fixture.user).await, Some(instant()));
    assert_eq!(sessions(&mut fixture.owner, fixture.user).await, (1, 1));
    fixture.remove().await;
}

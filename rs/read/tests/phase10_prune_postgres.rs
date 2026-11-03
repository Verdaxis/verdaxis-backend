//! Phase 10 Task 13: the product-analytics retention prune, against a
//! disposable PostgreSQL.
//!
//! Each test builds its own schema with the production shapes of the two fact
//! tables (`fixtures/prune_analytics.sql`) and drives the job through the same
//! entry point the command uses. The deployed role names are real roles of the
//! disposable server, so the identity and target refusals run for real.
#![cfg(feature = "contract-tests")]

use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;
use verdaxis_read::cli::context::RunContext;
use verdaxis_read::cli::jobs::prune::{self, Completion, PruneLimits, PruneReport, Runtime};
use verdaxis_read::cli::{Environment, Exit, IdentityArgs, JobArgs, JobKind};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
/// The job runs at 03:20 in Singapore, which is 19:20 UTC on the previous
/// calendar date. The retention window counts UTC dates.
const SINGAPORE_TIMER_HOUR_UTC: (u32, u32) = (19, 20);

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must name a disposable database"))
}

fn day(year: i32, month: u32, date: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, date).expect("a real calendar date")
}

fn instant(date: NaiveDate, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(hour, minute, 0).expect("a real time"))
}

fn with_database(url: &str, database: &str) -> String {
    let mut parsed = url::Url::parse(url).expect("parse a disposable URL");
    parsed.set_path(&format!("/{database}"));
    parsed.into()
}

fn database_of(url: &str) -> String {
    PgConnectOptions::from_str(url)
        .expect("parse a disposable URL")
        .get_database()
        .expect("the URL names a database")
        .to_owned()
}

/// One throwaway schema holding both fact tables, plus an owner connection for
/// seeding and for reading the result independently of the code under test.
struct Fixture {
    admin: PgConnection,
    schema: String,
}

impl Fixture {
    async fn create(admin_url: String) -> Self {
        let parsed = url::Url::parse(&admin_url).expect("parse the migrator URL");
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(
            parsed.path().ends_with("_test"),
            "only an explicitly disposable database may carry this fixture"
        );
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .expect("connect the migrator");
        let schema = format!("p10_prune_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .expect("create the schema");
        sqlx::raw_sql(include_str!("fixtures/prune_analytics.sql"))
            .execute(&mut admin)
            .await
            .expect("load the fact tables");
        // The deployed runtime privileges: login days are deletable, status
        // transitions are append-only (`deploy/postgres/app_acl_policy.sql`).
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO verdaxis_app, verdaxis_app_staging;
             GRANT SELECT ON users TO verdaxis_app, verdaxis_app_staging;
             GRANT SELECT, INSERT, UPDATE, DELETE ON user_login_days
                 TO verdaxis_app, verdaxis_app_staging;
             GRANT SELECT, INSERT ON user_status_transitions
                 TO verdaxis_app, verdaxis_app_staging;
             GRANT USAGE ON SEQUENCE prune_failure_attempts
                 TO verdaxis_app, verdaxis_app_staging"
        )))
        .execute(&mut admin)
        .await
        .expect("grant the runtime privileges");
        Self { admin, schema }
    }

    async fn new() -> Self {
        Self::create(env("RUST_TEST_MIGRATOR_DATABASE_URL")).await
    }

    /// A pool that opens no connection until it is used.
    fn pool(&self, url: &str) -> PgPool {
        let options = PgConnectOptions::from_str(url)
            .expect("parse a disposable URL")
            .options([("search_path", self.schema.as_str())]);
        PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy_with(options)
    }

    /// A second owner connection on the same schema, independent of `admin`.
    async fn second_connection(&self) -> PgConnection {
        let options = PgConnectOptions::from_str(&env("RUST_TEST_MIGRATOR_DATABASE_URL"))
            .expect("parse the migrator URL")
            .options([("search_path", self.schema.as_str())]);
        PgConnection::connect_with(&options)
            .await
            .expect("connect a second owner connection")
    }

    async fn seed(&mut self, users: i32, first: NaiveDate, last: NaiveDate) -> i64 {
        sqlx::query("SELECT seed_users($1)")
            .bind(users)
            .execute(&mut self.admin)
            .await
            .expect("seed users");
        sqlx::query_scalar("SELECT seed_login_days($1, $2)")
            .bind(first)
            .bind(last)
            .fetch_one(&mut self.admin)
            .await
            .expect("seed login days")
    }

    /// Like `seed`, with the rows inserted from the newest date to the oldest.
    async fn seed_newest_first(&mut self, users: i32, first: NaiveDate, last: NaiveDate) -> i64 {
        sqlx::query("SELECT seed_users($1)")
            .bind(users)
            .execute(&mut self.admin)
            .await
            .expect("seed users");
        sqlx::query_scalar("SELECT seed_login_days_newest_first($1, $2)")
            .bind(first)
            .bind(last)
            .fetch_one(&mut self.admin)
            .await
            .expect("seed login days newest first")
    }

    async fn login_ids(&mut self) -> Vec<Uuid> {
        sqlx::query_scalar("SELECT id FROM user_login_days ORDER BY id")
            .fetch_all(&mut self.admin)
            .await
            .expect("read login-day ids")
    }

    async fn login_dates(&mut self) -> Vec<NaiveDate> {
        sqlx::query_scalar("SELECT DISTINCT activity_date FROM user_login_days ORDER BY 1")
            .fetch_all(&mut self.admin)
            .await
            .expect("read login-day dates")
    }

    async fn login_count(&mut self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM user_login_days")
            .fetch_one(&mut self.admin)
            .await
            .expect("count login days")
    }

    async fn transition_ids(&mut self) -> Vec<Uuid> {
        sqlx::query_scalar("SELECT id FROM user_status_transitions ORDER BY id")
            .fetch_all(&mut self.admin)
            .await
            .expect("read transition ids")
    }

    /// First retained date as the database computes it, independent of the job.
    async fn database_cutoff(&mut self, today: NaiveDate) -> NaiveDate {
        sqlx::query_scalar("SELECT $1::date - 799")
            .bind(today)
            .fetch_one(&mut self.admin)
            .await
            .expect("compute the cutoff")
    }

    async fn finish(mut self) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.admin)
        .await
        .expect("drop the schema");
    }
}

/// One invocation: the command's expected identity, the runtime's configured
/// identity, and the target the write is attested against.
struct Invocation {
    kind: JobKind,
    expected_environment: Environment,
    expected_sha: String,
    configured_environment: &'static str,
    configured_sha: String,
    apply: bool,
    deploy_state: PathBuf,
    database: String,
}

impl Invocation {
    fn production() -> Self {
        Self {
            kind: JobKind::PruneAnalytics,
            expected_environment: Environment::Production,
            expected_sha: SHA.into(),
            configured_environment: "production",
            configured_sha: SHA.into(),
            apply: true,
            deploy_state: std::env::temp_dir()
                .join(format!("verdaxis-prune-{}.state", Uuid::new_v4().simple())),
            database: database_of(&env("RUST_TEST_PRODUCTION_DATABASE_URL")),
        }
    }

    fn job(&self) -> JobArgs {
        JobArgs {
            kind: self.kind,
            identity: IdentityArgs {
                environment: self.expected_environment,
                release_sha: self.expected_sha.clone(),
            },
            apply: self.apply,
        }
    }

    async fn execute(
        &self,
        pool: &PgPool,
        now: DateTime<Utc>,
        limits: PruneLimits,
    ) -> Result<PruneReport, Exit> {
        let runtime = Runtime {
            configured_environment: self.configured_environment,
            configured_release_sha: &self.configured_sha,
            deploy_state: &self.deploy_state,
            database: &self.database,
        };
        prune::execute(&self.job(), &runtime, pool, now, limits).await
    }

    async fn execute_default(
        &self,
        pool: &PgPool,
        now: DateTime<Utc>,
    ) -> Result<PruneReport, Exit> {
        self.execute(pool, now, PruneLimits::default()).await
    }
}

fn production_pool(fixture: &Fixture) -> PgPool {
    fixture.pool(&env("RUST_TEST_PRODUCTION_DATABASE_URL"))
}

/// Seed three users with one row per date from 900 dates before `today` to two
/// dates after it, run the prune at `now`, and check the exact window.
async fn assert_exact_window(today: NaiveDate, now: DateTime<Utc>) {
    let mut fixture = Fixture::new().await;
    let first = today - TimeDelta::days(900);
    let last = today + TimeDelta::days(2);
    fixture.seed(3, first, last).await;
    let cutoff = fixture.database_cutoff(today).await;
    let pool = production_pool(&fixture);

    let report = Invocation::production()
        .execute_default(&pool, now)
        .await
        .expect("the prune runs");

    // Dates before the cutoff are 900 down to 800 dates back: 101 dates.
    assert_eq!(report.cutoff, cutoff);
    assert_eq!(report.deleted, 3 * 101);
    assert_eq!(report.completion, Completion::Complete);
    let remaining = fixture.login_dates().await;
    let expected: Vec<NaiveDate> = (0..)
        .map(|offset| cutoff + TimeDelta::days(offset))
        .take_while(|date| *date <= last)
        .collect();
    assert_eq!(remaining, expected, "every date from the cutoff stays");
    let retained_through_today = remaining.iter().filter(|date| **date <= today).count();
    assert_eq!(retained_through_today, 800, "today and the previous 799");
    assert_eq!(fixture.login_count().await, 3 * (800 + 2));
    fixture.finish().await;
}

#[tokio::test]
async fn prune_keeps_exactly_800_utc_dates() {
    let today = day(2026, 7, 15);
    // Independent literal: 799 days before 2026-07-15.
    let mut probe = Fixture::new().await;
    assert_eq!(probe.database_cutoff(today).await, day(2024, 5, 7));
    probe.finish().await;

    // At the Singapore timer time the UTC date is still 15 July, although it
    // is already 16 July in Singapore.
    let (hour, minute) = SINGAPORE_TIMER_HOUR_UTC;
    assert_exact_window(today, instant(today, hour, minute)).await;
    // Midnight UTC starts the next date.
    assert_exact_window(day(2026, 7, 16), instant(day(2026, 7, 16), 0, 0)).await;
    // A window that spans a leap day.
    assert_exact_window(day(2025, 12, 31), instant(day(2025, 12, 31), 12, 0)).await;
}

#[tokio::test]
async fn prune_never_deletes_status_transitions() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(2, today - TimeDelta::days(900), today).await;
    for effective in [
        "2020-01-01T00:00:00Z",
        "2024-01-01T00:00:00Z",
        "2026-07-15T00:00:00Z",
    ] {
        let seeded: i64 = sqlx::query_scalar("SELECT seed_status_transitions($1::timestamptz)")
            .bind(effective)
            .fetch_one(&mut fixture.admin)
            .await
            .expect("seed transitions");
        assert_eq!(seeded, 2);
    }
    let before = fixture.transition_ids().await;
    assert_eq!(before.len(), 6);
    let pool = production_pool(&fixture);

    let report = Invocation::production()
        .execute_default(&pool, instant(today, 12, 0))
        .await
        .expect("the prune runs");

    assert!(report.deleted > 0, "login days past the window were pruned");
    assert_eq!(fixture.transition_ids().await, before);

    // The deployed role cannot delete transitions at all.
    let refused = sqlx::query("DELETE FROM user_status_transitions")
        .execute(&pool)
        .await
        .expect_err("transitions are append-only for the runtime role");
    assert_eq!(
        refused
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    assert_eq!(fixture.transition_ids().await, before);
    fixture.finish().await;
}

#[tokio::test]
async fn prune_refuses_wrong_environment_or_sha() {
    let today = day(2026, 7, 15);
    let now = instant(today, 12, 0);
    let mut fixture = Fixture::new().await;
    fixture.seed(2, today - TimeDelta::days(850), today).await;
    let before = fixture.login_ids().await;

    // A same-named schema in the other database, to prove the database arm.
    let wrong_url = env("RUST_TEST_WRONG_DATABASE_URL");
    let mut elsewhere = Fixture::create(with_database(
        &env("RUST_TEST_MIGRATOR_DATABASE_URL"),
        &database_of(&wrong_url),
    ))
    .await;
    elsewhere.seed(2, today - TimeDelta::days(850), today).await;
    let elsewhere_before = elsewhere.login_ids().await;

    // Refusals that need no database: the pool must never open a connection.
    let idle = production_pool(&fixture);
    let mut wrong_environment = Invocation::production();
    wrong_environment.configured_environment = "staging";
    let mut other_sha = Invocation::production();
    other_sha.configured_sha = "f".repeat(40);
    let mut uppercase_sha = Invocation::production();
    uppercase_sha.configured_sha = SHA.to_uppercase();
    let mut empty_path = Invocation::production();
    empty_path.deploy_state = PathBuf::new();
    let mut relative_path = Invocation::production();
    relative_path.deploy_state = PathBuf::from(".runtime-deploy/production.state");
    let mut expects_staging = Invocation::production();
    expects_staging.expected_environment = Environment::Staging;
    let live_deploy = Invocation::production();
    std::fs::write(&live_deploy.deploy_state, b"restart-authorized").expect("write deploy state");
    let dangling_deploy = Invocation::production();
    std::os::unix::fs::symlink(
        std::env::temp_dir().join(format!("absent-{}", Uuid::new_v4().simple())),
        &dangling_deploy.deploy_state,
    )
    .expect("plant a dangling deploy state link");
    for (label, invocation) in [
        ("configured environment differs", &wrong_environment),
        ("configured release differs", &other_sha),
        ("uppercase release", &uppercase_sha),
        ("deploy state path is empty", &empty_path),
        ("deploy state path is relative", &relative_path),
        ("expected environment differs", &expects_staging),
        ("deploy state present", &live_deploy),
        ("deploy state is a dangling link", &dangling_deploy),
    ] {
        assert_eq!(
            invocation.execute_default(&idle, now).await,
            Err(Exit::Precondition),
            "{label}"
        );
    }
    assert_eq!(idle.size(), 0, "a refusal opens no database connection");

    let mut without_apply = Invocation::production();
    without_apply.apply = false;
    assert_eq!(
        without_apply.execute_default(&idle, now).await,
        Err(Exit::Usage)
    );
    let mut other_job = Invocation::production();
    other_job.kind = JobKind::News;
    assert_eq!(
        other_job.execute_default(&idle, now).await,
        Err(Exit::Usage)
    );
    // The expected release must be a full lowercase SHA, even when the runtime
    // reports the same malformed value (an empty pair would compare equal).
    for (label, expected, configured) in [
        ("abbreviated", SHA[..7].to_owned(), SHA.to_owned()),
        ("uppercase", SHA.to_uppercase(), SHA.to_uppercase()),
        ("empty", String::new(), String::new()),
        ("too long", format!("{SHA}0"), format!("{SHA}0")),
    ] {
        let mut malformed = Invocation::production();
        malformed.expected_sha = expected;
        malformed.configured_sha = configured;
        assert_eq!(
            malformed.execute_default(&idle, now).await,
            Err(Exit::Usage),
            "{label} expected release"
        );
    }
    assert_eq!(
        idle.size(),
        0,
        "a usage refusal opens no database connection"
    );

    // Refusals that need the writing connection: identity matches, target does not.
    let staging_pool = fixture.pool(&env("RUST_TEST_STAGING_DATABASE_URL"));
    assert_eq!(
        Invocation::production()
            .execute_default(&staging_pool, now)
            .await,
        Err(Exit::Precondition),
        "the staging role is not the production role"
    );
    let production_pool_for_staging = production_pool(&fixture);
    let mut staging_identity = Invocation::production();
    staging_identity.expected_environment = Environment::Staging;
    staging_identity.configured_environment = "staging";
    assert_eq!(
        staging_identity
            .execute_default(&production_pool_for_staging, now)
            .await,
        Err(Exit::Precondition),
        "the production role is not the staging role"
    );
    let wrong_database_pool = elsewhere.pool(&wrong_url);
    assert_eq!(
        Invocation::production()
            .execute_default(&wrong_database_pool, now)
            .await,
        Err(Exit::Precondition),
        "the production role on another database is refused"
    );

    // Nothing was deleted by any refusal.
    assert_eq!(fixture.login_ids().await, before);
    assert_eq!(elsewhere.login_ids().await, elsewhere_before);

    // Control: the same data is pruned once identity and target match, so the
    // refusals above are not a broken harness.
    let report = Invocation::production()
        .execute_default(&production_pool(&fixture), now)
        .await
        .expect("a matching invocation prunes");
    assert_eq!(report.deleted, 2 * 51);
    let _ = std::fs::remove_file(&live_deploy.deploy_state);
    let _ = std::fs::remove_file(&dangling_deploy.deploy_state);
    fixture.finish().await;
    elsewhere.finish().await;
}

#[tokio::test]
async fn prune_is_idempotent_on_the_same_day() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(3, today - TimeDelta::days(850), today).await;
    let pool = production_pool(&fixture);
    let invocation = Invocation::production();

    let first = invocation
        .execute_default(&pool, instant(today, 1, 0))
        .await
        .expect("first run");
    assert_eq!(first.deleted, 3 * 51);
    let after_first = fixture.login_ids().await;

    for (hour, minute) in [(1, 0), (23, 59)] {
        let again = invocation
            .execute_default(&pool, instant(today, hour, minute))
            .await
            .expect("a rerun on the same date");
        assert_eq!(again.deleted, 0);
        assert_eq!(again.completion, Completion::Complete);
        assert_eq!(fixture.login_ids().await, after_first);
    }

    // The next UTC date moves the window by exactly one date.
    let next = invocation
        .execute_default(&pool, instant(today + TimeDelta::days(1), 0, 0))
        .await
        .expect("the next date");
    assert_eq!(next.deleted, 3);
    assert_eq!(
        fixture.login_dates().await.first().copied(),
        Some(today - TimeDelta::days(798))
    );
    fixture.finish().await;
}

#[tokio::test]
async fn prune_deletes_in_bounded_batches_oldest_first() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    // One user: six dates past the window (805 down to 800 dates back). The
    // rows are inserted newest date first, so only an explicit ordering removes
    // the oldest dates first.
    fixture
        .seed_newest_first(1, today - TimeDelta::days(805), today)
        .await;
    let pool = production_pool(&fixture);
    let invocation = Invocation::production();
    let now = instant(today, 12, 0);
    let two_by_two = PruneLimits {
        batch_rows: 2,
        max_batches: 2,
        deadline: Duration::from_secs(60),
        ..PruneLimits::default()
    };

    let first = invocation
        .execute(&pool, now, two_by_two)
        .await
        .expect("first bounded run");
    assert_eq!((first.deleted, first.batches), (4, 2));
    assert_eq!(first.completion, Completion::LimitReached);
    assert_eq!(
        fixture.login_dates().await.first().copied(),
        Some(today - TimeDelta::days(801)),
        "the oldest dates go first; two past-window dates remain"
    );

    let second = invocation
        .execute(&pool, now, two_by_two)
        .await
        .expect("second bounded run");
    assert_eq!(second.deleted, 2);
    assert_eq!(second.completion, Completion::Complete);
    assert_eq!(
        fixture.login_dates().await.first().copied(),
        Some(today - TimeDelta::days(799))
    );
    assert_eq!(fixture.login_count().await, 800);

    // An exhausted deadline starts no batch and deletes nothing.
    let mut late = Fixture::new().await;
    late.seed(1, today - TimeDelta::days(805), today).await;
    let late_pool = production_pool(&late);
    let expired = invocation
        .execute(
            &late_pool,
            now,
            PruneLimits {
                batch_rows: 2,
                max_batches: 10,
                deadline: Duration::ZERO,
                ..PruneLimits::default()
            },
        )
        .await
        .expect("an expired deadline is not a failure");
    assert_eq!((expired.deleted, expired.batches), (0, 0));
    assert_eq!(expired.completion, Completion::LimitReached);
    assert_eq!(late.login_count().await, 806);

    fixture.finish().await;
    late.finish().await;
}

#[tokio::test]
async fn prune_skips_rows_another_transaction_holds() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(1, today - TimeDelta::days(805), today).await;
    let oldest = today - TimeDelta::days(805);
    let pool = production_pool(&fixture);
    let invocation = Invocation::production();
    let now = instant(today, 12, 0);

    let mut holder = fixture.second_connection().await;
    sqlx::query("BEGIN")
        .execute(&mut holder)
        .await
        .expect("begin");
    sqlx::query("SELECT id FROM user_login_days WHERE activity_date = $1 FOR UPDATE")
        .bind(oldest)
        .fetch_one(&mut holder)
        .await
        .expect("lock the oldest row");

    // The prune does not wait for the lock: it removes every other old row.
    let report = tokio::time::timeout(
        Duration::from_secs(20),
        invocation.execute_default(&pool, now),
    )
    .await
    .expect("the prune does not block on a locked row")
    .expect("the prune runs");
    assert_eq!(report.deleted, 5);
    assert_eq!(fixture.login_dates().await.first().copied(), Some(oldest));

    sqlx::query("ROLLBACK")
        .execute(&mut holder)
        .await
        .expect("release the lock");
    let rerun = invocation
        .execute_default(&pool, now)
        .await
        .expect("rerun after release");
    assert_eq!(rerun.deleted, 1);
    assert_eq!(fixture.login_count().await, 800);
    fixture.finish().await;
}

#[tokio::test]
async fn overlapping_runs_delete_each_row_once_and_keep_the_window() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(3, today - TimeDelta::days(1300), today).await;
    let pool = production_pool(&fixture);
    let now = instant(today, 12, 0);
    let small = PruneLimits {
        batch_rows: 100,
        ..PruneLimits::default()
    };
    let (one, two) = (Invocation::production(), Invocation::production());

    let (first, second) = tokio::join!(
        one.execute(&pool, now, small),
        two.execute(&pool, now, small)
    );

    let deleted = first.expect("first run").deleted + second.expect("second run").deleted;
    assert_eq!(
        deleted,
        3 * (1300 - 799),
        "every old row is deleted exactly once"
    );
    assert_eq!(fixture.login_count().await, 3 * 800);
    assert_eq!(
        fixture.login_dates().await.first().copied(),
        Some(today - TimeDelta::days(799))
    );
    fixture.finish().await;
}

/// Install a statement trigger that raises `sqlstate` for the first
/// `failures` delete attempts, then return the number of attempts seen.
async fn failing_fixture(failures: i64, sqlstate: &str) -> (Fixture, NaiveDate) {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(1, today - TimeDelta::days(805), today).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER fail_before_delete BEFORE DELETE ON user_login_days
         FOR EACH STATEMENT EXECUTE FUNCTION fail_first_attempts('{failures}', '{sqlstate}')"
    )))
    .execute(&mut fixture.admin)
    .await
    .expect("install the failure trigger");
    (fixture, today)
}

async fn attempts(fixture: &mut Fixture) -> i64 {
    sqlx::query_scalar(
        "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM prune_failure_attempts",
    )
    .fetch_one(&mut fixture.admin)
    .await
    .expect("read the attempt counter")
}

#[tokio::test]
async fn serialization_failure_is_retried_then_succeeds() {
    let (mut fixture, today) = failing_fixture(1, "40001").await;
    let pool = production_pool(&fixture);

    let report = Invocation::production()
        .execute_default(&pool, instant(today, 12, 0))
        .await
        .expect("the second attempt succeeds");

    assert_eq!(report.deleted, 6);
    assert_eq!(attempts(&mut fixture).await, 2);
    fixture.finish().await;
}

#[tokio::test]
async fn deadlock_is_retried_and_stops_after_three_attempts() {
    let (mut fixture, today) = failing_fixture(2, "40P01").await;
    let pool = production_pool(&fixture);
    let report = Invocation::production()
        .execute_default(&pool, instant(today, 12, 0))
        .await
        .expect("the third attempt succeeds");
    assert_eq!(report.deleted, 6);
    assert_eq!(attempts(&mut fixture).await, 3);
    fixture.finish().await;

    // Three failures exhaust the bounded retry: no fourth attempt, nothing deleted.
    let (mut fixture, today) = failing_fixture(3, "40P01").await;
    let pool = production_pool(&fixture);
    assert_eq!(
        Invocation::production()
            .execute_default(&pool, instant(today, 12, 0))
            .await,
        Err(Exit::Dependency)
    );
    assert_eq!(attempts(&mut fixture).await, 3);
    assert_eq!(fixture.login_count().await, 806);
    fixture.finish().await;
}

#[tokio::test]
async fn other_database_errors_are_not_retried() {
    let (mut fixture, today) = failing_fixture(100, "23514").await;
    let pool = production_pool(&fixture);
    assert_eq!(
        Invocation::production()
            .execute_default(&pool, instant(today, 12, 0))
            .await,
        Err(Exit::Dependency)
    );
    assert_eq!(attempts(&mut fixture).await, 1);
    assert_eq!(fixture.login_count().await, 806);
    fixture.finish().await;
}

#[tokio::test]
async fn context_entry_prunes_and_reports_success() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(1, today - TimeDelta::days(805), today).await;
    let invocation = Invocation::production();
    let context = RunContext {
        environment: Environment::Production,
        release_sha: SHA.into(),
        now: instant(today, 12, 0),
        pool: production_pool(&fixture),
        migrator: None,
    };

    let exit = prune::run_with_context(
        &invocation.job(),
        &context,
        &invocation.deploy_state,
        &invocation.database,
    )
    .await;

    assert_eq!(exit, Exit::Ok);
    assert_eq!(fixture.login_count().await, 800);
    fixture.finish().await;
}

#[tokio::test]
async fn delete_gives_up_when_a_table_lock_is_held() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(1, today - TimeDelta::days(805), today).await;
    let pool = production_pool(&fixture);
    let invocation = Invocation::production();
    let now = instant(today, 12, 0);
    let impatient = PruneLimits {
        lock_timeout: Duration::from_millis(500),
        ..PruneLimits::default()
    };

    // A schema change holds the table exclusively.
    let mut holder = fixture.second_connection().await;
    sqlx::query("BEGIN")
        .execute(&mut holder)
        .await
        .expect("begin");
    sqlx::query("LOCK TABLE user_login_days IN ACCESS EXCLUSIVE MODE")
        .execute(&mut holder)
        .await
        .expect("lock the table");

    let started = std::time::Instant::now();
    let refused = tokio::time::timeout(
        Duration::from_secs(30),
        invocation.execute(&pool, now, impatient),
    )
    .await
    .expect("the delete does not wait for the lock without bound");
    assert_eq!(refused, Err(Exit::Dependency));
    assert!(started.elapsed() < Duration::from_secs(15));

    sqlx::query("ROLLBACK")
        .execute(&mut holder)
        .await
        .expect("release the lock");
    assert_eq!(fixture.login_count().await, 806, "nothing was deleted");
    // The refusal leaves the pooled connection usable: a rerun prunes.
    let rerun = invocation
        .execute(&pool, now, impatient)
        .await
        .expect("rerun after release");
    assert_eq!(rerun.deleted, 6);
    fixture.finish().await;
}

#[tokio::test]
async fn delete_stops_at_the_run_deadline() {
    let today = day(2026, 7, 15);
    let mut fixture = Fixture::new().await;
    fixture.seed(1, today - TimeDelta::days(805), today).await;
    // Every delete statement sleeps far longer than the run allows.
    sqlx::raw_sql(
        "CREATE TRIGGER slow_before_delete BEFORE DELETE ON user_login_days
         FOR EACH STATEMENT EXECUTE FUNCTION slow_delete()",
    )
    .execute(&mut fixture.admin)
    .await
    .expect("install the slow trigger");
    let pool = production_pool(&fixture);
    let short_run = PruneLimits {
        deadline: Duration::from_secs(1),
        ..PruneLimits::default()
    };

    let started = std::time::Instant::now();
    let result = Invocation::production()
        .execute(&pool, instant(today, 12, 0), short_run)
        .await;

    assert_eq!(result, Err(Exit::Dependency));
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the statement timeout, not the trigger, ended the delete"
    );
    assert_eq!(attempts(&mut fixture).await, 1, "a timeout is not retried");
    assert_eq!(fixture.login_count().await, 806);
    fixture.finish().await;
}

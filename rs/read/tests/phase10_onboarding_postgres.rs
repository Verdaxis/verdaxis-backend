//! Phase 10 Task 16: the onboarding candidate projection on PostgreSQL.
//!
//! Each case builds its own schema in the disposable database and reads it as
//! the runtime role with SELECT only, so a monitor that needed a write grant
//! would fail here. Needs `RUST_TEST_MIGRATOR_DATABASE_URL` and
//! `RUST_TEST_DATABASE_URL`.
#![cfg(feature = "contract-tests")]
use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::jobs::onboarding::{
    AlertSender, CandidateSource, Mode, Options, Outcome, PgCandidates, SendError, run,
};

const SCHEMA_SQL: &str = include_str!("fixtures/phase10_onboarding_schema.sql");

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 4, 8, 0, 0).unwrap()
}

fn before(seconds: i64) -> DateTime<Utc> {
    now() - Duration::seconds(seconds)
}

struct Fixture {
    admin: PgConnection,
    pool: PgPool,
    schema: String,
}

impl Fixture {
    async fn new() -> Self {
        let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL");
        let runtime =
            std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
        assert!(
            url::Url::parse(&migrator)
                .unwrap()
                .path()
                .ends_with("_test")
        );
        let mut admin = PgConnection::connect(&migrator).await.unwrap();
        let schema = format!("p10_onboarding_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(SCHEMA_SQL).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app; \
             GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app;"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |connection, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&runtime)
            .await
            .unwrap();
        Self {
            admin,
            pool,
            schema,
        }
    }

    async fn execute(&mut self, sql: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn organization(&mut self, id: Uuid, name: &str, status: &str, provenance: &str) {
        sqlx::query("INSERT INTO organizations VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(name)
            .bind(status)
            .bind(provenance)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn user(
        &mut self,
        id: Uuid,
        email: &str,
        role: Option<&str>,
        status: &str,
        organization: Option<Uuid>,
        created_at: DateTime<Utc>,
        verified: bool,
        last_login: Option<DateTime<Utc>>,
    ) {
        sqlx::query(
            "INSERT INTO users (id, email, first_name, last_name, role, status, \
             organization_id, created_at, email_verified, last_login) \
             VALUES ($1, $2, 'Pat', 'Lee', $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(email)
        .bind(role)
        .bind(status)
        .bind(organization)
        .bind(created_at)
        .bind(verified)
        .bind(last_login)
        .execute(&mut self.admin)
        .await
        .unwrap();
    }

    async fn finish(mut self) {
        self.pool.close().await;
        let schema = self.schema.clone();
        self.execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    }
}

#[derive(Default)]
struct Recorder {
    messages: Mutex<Vec<String>>,
    delay: Option<std::time::Duration>,
}

impl AlertSender for Recorder {
    async fn send(&self, message: &str) -> Result<(), SendError> {
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        self.messages.lock().unwrap().push(message.to_owned());
        Ok(())
    }
}

struct StateDirectory(PathBuf);

impl StateDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "verdaxis-onboarding-pg-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn options(&self, mode: Mode) -> Options {
        Options {
            state_file: self.0.join("state.json"),
            mode,
        }
    }
}

impl Drop for StateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn onboarding_candidates_match_the_python_projection() {
    let mut fixture = Fixture::new().await;
    let approved = Uuid::from_u128(0x100);
    let rejected = Uuid::from_u128(0x101);
    let demo = Uuid::from_u128(0x102);
    fixture
        .organization(approved, "Example Energy", "APPROVED", "REAL")
        .await;
    fixture
        .organization(rejected, "Refused Co", "REJECTED", "REAL")
        .await;
    fixture
        .organization(demo, "Demo Co", "APPROVED", "DEMO")
        .await;

    let first_login = Uuid::from_u128(1);
    let refused = Uuid::from_u128(2);
    let in_demo = Uuid::from_u128(3);
    let administrator = Uuid::from_u128(4);
    let no_role = Uuid::from_u128(5);
    let canary = Uuid::from_u128(6);
    let logged_in = Uuid::from_u128(7);
    fixture
        .user(
            first_login,
            "first@example.com",
            Some("BUYER"),
            "APPROVED",
            Some(approved),
            before(86_400),
            true,
            None,
        )
        .await;
    fixture
        .user(
            refused,
            "refused@example.com",
            Some("SUPPLIER"),
            "APPROVED",
            Some(rejected),
            before(86_400),
            true,
            None,
        )
        .await;
    fixture
        .user(
            in_demo,
            "demo@example.com",
            Some("BUYER"),
            "APPROVED",
            Some(demo),
            before(86_400),
            true,
            None,
        )
        .await;
    fixture
        .user(
            administrator,
            "admin@example.com",
            Some("ADMIN"),
            "APPROVED",
            None,
            before(86_400),
            true,
            None,
        )
        .await;
    fixture
        .user(
            no_role,
            "norole@example.com",
            None,
            "APPROVED",
            None,
            before(86_400),
            true,
            None,
        )
        .await;
    fixture
        .user(
            canary,
            "canary+prod@prod.canary.verdaxis.exchange",
            Some("BUYER"),
            "APPROVED",
            Some(approved),
            before(86_400),
            true,
            None,
        )
        .await;
    fixture
        .user(
            logged_in,
            "active@example.com",
            Some("BUYER"),
            "APPROVED",
            Some(approved),
            before(86_400),
            true,
            Some(before(60)),
        )
        .await;
    // The active user has completed every gate, so only the missing login
    // separates it from a first-login alert.
    sqlx::query("INSERT INTO organization_join_requests VALUES ($1, $2, $3, 'APPROVED', $4, $4)")
        .bind(Uuid::new_v4())
        .bind(logged_in)
        .bind(approved)
        .bind(before(5 * 3600))
        .execute(&mut fixture.admin)
        .await
        .unwrap();
    // Membership, user approvals (the latest APPROVED transition only) and the
    // organization approval audit row feed the first-login time.
    sqlx::query("INSERT INTO organization_join_requests VALUES ($1, $2, $3, 'APPROVED', $4, $4)")
        .bind(Uuid::new_v4())
        .bind(first_login)
        .bind(approved)
        .bind(before(5 * 3600))
        .execute(&mut fixture.admin)
        .await
        .unwrap();
    for (to_status, effective_at) in [
        ("APPROVED", before(4 * 3600)),
        ("APPROVED", before(3 * 3600)),
        ("PENDING", before(60)),
    ] {
        sqlx::query("INSERT INTO user_status_transitions VALUES ($1, $2, $3, $4)")
            .bind(Uuid::new_v4())
            .bind(first_login)
            .bind(to_status)
            .bind(effective_at)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
    }
    for (action, resource, at) in [
        (
            "admin.organization_approved",
            approved.to_string(),
            before(7200 + 30),
        ),
        (
            "admin.organization_approved",
            approved.to_string(),
            before(6 * 3600),
        ),
        ("admin.user_approved", approved.to_string(), before(60)),
        (
            "admin.organization_approved",
            Uuid::from_u128(0x999).to_string(),
            before(10),
        ),
    ] {
        sqlx::query("INSERT INTO audit_logs VALUES ($1, $2, $3, $4)")
            .bind(Uuid::new_v4())
            .bind(action)
            .bind(resource)
            .bind(at)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
    }
    // One unused and one used pending registration.
    let pending = Uuid::from_u128(0x200);
    sqlx::query("INSERT INTO pending_registrations VALUES ($1, 'pending@example.com', NULL, 'Lee', 'BUYER', $2, NULL, $3)")
        .bind(pending).bind(now() + Duration::minutes(4)).bind(before(600))
        .execute(&mut fixture.admin).await.unwrap();
    sqlx::query("INSERT INTO pending_registrations VALUES ($1, 'used@example.com', NULL, NULL, 'BUYER', $2, $3, $4)")
        .bind(Uuid::from_u128(0x201)).bind(now() + Duration::minutes(4)).bind(before(10)).bind(before(600))
        .execute(&mut fixture.admin).await.unwrap();

    let candidates = PgCandidates::new(fixture.pool.clone())
        .load()
        .await
        .expect("load the candidates");
    let keys: Vec<&str> = candidates.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(candidates.len(), 6, "{keys:?}");
    assert!(
        !keys
            .iter()
            .any(|key| key.contains(&administrator.to_string()))
    );
    assert!(
        !keys.iter().any(|key| key.contains(&no_role.to_string())),
        "a NULL role is not selected"
    );
    assert!(
        !keys.iter().any(|key| key.contains("201")),
        "a used registration is not a candidate"
    );

    let first = candidates
        .iter()
        .find(|c| c.key == format!("user:{first_login}"))
        .unwrap();
    assert_eq!(first.email, "first@example.com");
    assert_eq!(first.role.as_deref(), Some("BUYER"));
    assert_eq!(first.user_status.as_deref(), Some("APPROVED"));
    assert_eq!(first.organization_name.as_deref(), Some("Example Energy"));
    assert_eq!(first.organization_status.as_deref(), Some("APPROVED"));
    assert_eq!(first.organization_provenance.as_deref(), Some("REAL"));
    assert_eq!(first.membership_status.as_deref(), Some("APPROVED"));
    assert_eq!(first.membership_reviewed_at, Some(before(5 * 3600)));
    assert_eq!(
        first.user_approved_at,
        Some(before(3 * 3600)),
        "latest APPROVED transition"
    );
    assert_eq!(
        first.organization_approved_at,
        Some(before(7200 + 30)),
        "latest organization approval row"
    );
    assert_eq!(first.pending_registration_expires_at, None);
    assert!(first.email_verified);

    let pending_candidate = candidates
        .iter()
        .find(|c| c.key == format!("pending:{pending}"))
        .unwrap();
    assert_eq!(
        pending_candidate.pending_registration_expires_at,
        Some(now() + Duration::minutes(4))
    );
    assert!(!pending_candidate.email_verified);
    assert_eq!(pending_candidate.user_status, None);
    assert_eq!(pending_candidate.first_name, None);
    assert_eq!(pending_candidate.last_name.as_deref(), Some("Lee"));

    // The full run: the stages Python would report, in a stable order.
    let state = StateDirectory::new();
    let sender = Recorder::default();
    let outcome = run(
        &PgCandidates::new(fixture.pool.clone()),
        &sender,
        &state.options(Mode::Monitor),
        now(),
    )
    .await
    .unwrap();
    let messages = sender.messages.lock().unwrap().clone();
    assert_eq!(
        outcome,
        Outcome::Checked {
            candidates: 6,
            alerts: 3,
            recoveries: 0
        },
        "{messages:?}"
    );
    let stages: Vec<&str> = messages
        .iter()
        .map(|m| m.lines().find(|l| l.starts_with("Stage: ")).unwrap())
        .collect();
    assert_eq!(
        stages,
        [
            "Stage: First login overdue",
            "Stage: Rejected",
            "Stage: Organization setup expiring"
        ]
    );
    fixture.finish().await;
}

#[tokio::test]
async fn onboarding_candidate_load_is_bounded_and_read_only() {
    let mut fixture = Fixture::new().await;
    for number in 1..=3_u128 {
        fixture
            .user(
                Uuid::from_u128(number),
                &format!("u{number}@example.com"),
                Some("BUYER"),
                "PENDING",
                None,
                before(60),
                false,
                None,
            )
            .await;
    }
    assert_eq!(
        PgCandidates::with_limit(fixture.pool.clone(), 2)
            .load()
            .await
            .unwrap_err(),
        Exit::Precondition,
        "more rows than the bound refuses instead of truncating"
    );
    assert_eq!(
        PgCandidates::with_limit(fixture.pool.clone(), 3)
            .load()
            .await
            .unwrap()
            .len(),
        3
    );
    // The runtime role has no write grant, and the load needed none.
    let write = sqlx::query("UPDATE users SET last_login = now()")
        .execute(&fixture.pool)
        .await;
    assert!(write.is_err(), "the fixture role is read-only");
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn onboarding_postgres_three_runs_send_once() {
    let mut fixture = Fixture::new().await;
    let organization = Uuid::from_u128(0x100);
    fixture
        .organization(organization, "Example Energy", "APPROVED", "REAL")
        .await;
    fixture
        .user(
            Uuid::from_u128(1),
            "first@example.com",
            Some("BUYER"),
            "APPROVED",
            Some(organization),
            before(86_400),
            true,
            None,
        )
        .await;
    let state = Arc::new(StateDirectory::new());
    let sender = Arc::new(Recorder {
        delay: Some(std::time::Duration::from_millis(600)),
        ..Recorder::default()
    });
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut runs = Vec::new();
    for _ in 0..3 {
        // Each run has its own pool, so the three database sessions are independent.
        let pool = fixture.pool.clone();
        let (state, sender, barrier) = (
            Arc::clone(&state),
            Arc::clone(&sender),
            Arc::clone(&barrier),
        );
        runs.push(tokio::spawn(async move {
            barrier.wait().await;
            run(
                &PgCandidates::new(pool),
                &*sender,
                &state.options(Mode::Monitor),
                now(),
            )
            .await
        }));
    }
    let mut outcomes = Vec::new();
    for handle in runs {
        outcomes.push(handle.await.unwrap().unwrap());
    }
    assert_eq!(
        outcomes.iter().filter(|o| **o == Outcome::LockBusy).count(),
        2,
        "{outcomes:?}"
    );
    assert_eq!(sender.messages.lock().unwrap().len(), 1);
    fixture.finish().await;
}

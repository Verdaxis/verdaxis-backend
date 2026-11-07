//! PostgreSQL behavior of the account offboarding command and its preview.
//!
//! These cases are ignored in a plain `cargo test`.
//! `rs/read/tests/run_offboarding_command.sh` migrates a disposable database to
//! the Alembic head, applies the runtime ACL bundle and runs them with
//! `--include-ignored --test-threads=1`:
//!
//! - `RUST_TEST_DATABASE_URL`: the runtime application role;
//! - `RUST_TEST_MIGRATOR_DATABASE_URL`: the schema owner, used only for
//!   fixtures, the test hook tables and observation.
//!
//! The market hook here is a test double that records its call; the command's
//! own account, credential, audit and outbox writes are real. Every race uses
//! independent connections and observes the lock wait in `pg_stat_activity`.
#![cfg(feature = "contract-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;
use verdaxis_read::{
    AppState, Config,
    auth::{
        VerificationSettings, VerifiedAccess,
        offboarding::{CUTOFF_SKEW_MARGIN_SECONDS, UnusablePasswordHash},
        token_bytes::{generate_token, token_digest},
        verify_access,
    },
    business::{
        admission::{AdmissionEvents, AuditContext},
        error::refusal,
        offboarding::{
            MarketInvalidationSummary, OffboardAccount, OffboardRequest, OffboardingMarketHook,
            UserMarketScope,
        },
        transaction::{
            AttemptError, CommandFuture, CompletionControl, CompletionPoint, run_controlled_write,
            run_ordered_write,
        },
    },
    compiled_sha, connect,
    runtime::phase4::offboarding_routes,
};

const SECRET: &str = "synthetic-offboarding-key-at-least-32-characters";
const ISSUER: &str = "offboarding-test";
const AUDIENCE: &str = "offboarding-test";
const CASE_REFERENCE: &str = "CASE-2026/0042";
const IGNORE: &str = "requires the migrated disposable PostgreSQL harness";
/// The longest a test waits to see a command blocked on a row lock. The
/// runtime pool's own lock timeout is three seconds, so the holder releases
/// well before it.
const WAIT_OBSERVATION: Duration = Duration::from_secs(2);
/// Canonical catalog rows the migrations create: Bio Methanol and Singapore.
const PRODUCT: Uuid = Uuid::from_u128(0x9510c713_6e39_5080_add3_0a7c29657b79);
const DELIVERY_POINT: Uuid = Uuid::from_u128(0x73835e92_820e_584b_8280_bb61c63aa28e);

// ---------------------------------------------------------------------------
// Harness.
// ---------------------------------------------------------------------------

fn disposable(variable: &str) -> String {
    let raw = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable} must name a disposable database"));
    let parsed = url::Url::parse(&raw).expect("valid disposable URL");
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.port().is_some_and(|port| port != 5432));
    assert!(parsed.path().ends_with("_test"));
    raw
}

struct Harness {
    state: AppState,
    /// The schema owner: fixtures and observation only.
    owner: PgPool,
    /// The runtime role, for direct commands and concurrent writers.
    runtime: PgPool,
    runtime_role: String,
    router: Router,
}

struct Seed {
    organization: Uuid,
    other_organization: Uuid,
    admin: Uuid,
    subject: Uuid,
    join: Uuid,
}

impl Harness {
    async fn open() -> Self {
        let runtime_url = disposable("RUST_TEST_DATABASE_URL");
        let owner_url = disposable("RUST_TEST_MIGRATOR_DATABASE_URL");
        let runtime_role = url::Url::parse(&runtime_url).unwrap().username().to_owned();
        let owner = PgPoolOptions::new()
            .max_connections(3)
            .connect(&owner_url)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/offboarding_command_harness.sql"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM offboarding_test_failpoints")
            .execute(&owner)
            .await
            .unwrap();
        let runtime = PgPoolOptions::new()
            .max_connections(4)
            .connect(&runtime_url)
            .await
            .unwrap();
        let config = Config {
            kyc_execution_gate_enabled: false,
            db_session_time_zone: "UTC".into(),
            environment: "test".into(),
            release_sha: compiled_sha().into(),
            migration_revision: ALLOWED_REVISIONS[0].into(),
            database_url: runtime_url,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 4,
            pool_max: 5,
            python_budget: 52,
            configured_max_connections: 100,
            compliance_eur_usd_rate: None,
            fleet_artifact_path: "data/fleet_demand.json".into(),
            prepared_budget: None,
            phase2: serde_json::from_value(json!({
                "jwt_secret": SECRET, "jwt_issuer": ISSUER, "jwt_audience": AUDIENCE,
                "frontend_url": "https://app.example.test", "email_from": "team@example.test",
                "resend_api_key": "synthetic", "auth_domain_datasets": "null"
            }))
            .unwrap(),
        };
        let pools = connect(&config).await.unwrap();
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let state = AppState::new(config, pools, policy).unwrap();
        let router = offboarding_routes(&state, Arc::new(RecordingHook))
            .expect("the composition has Phase 2 settings");
        Self {
            state,
            owner,
            runtime,
            runtime_role,
            router,
        }
    }

    async fn organization(&self, kind: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, name, type, verification_status, provenance) \
             VALUES ($1, $2, $3, 'APPROVED', 'REAL')",
        )
        .bind(id)
        .bind(format!("Synthetic offboarding company {id}"))
        .bind(kind)
        .execute(&self.owner)
        .await
        .unwrap();
        id
    }

    async fn user(&self, role: &str, organization: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, status, organization_id, \
             email_verified, must_change_password, kyc_status, kyc_organization_id, \
             password_reset_token_hash, password_reset_expires) \
             VALUES ($1, $2, 'synthetic-unused-hash', $3, 'APPROVED', $4, true, false, \
             CASE WHEN $4::uuid IS NULL THEN 'PENDING' ELSE 'APPROVED' END, $4, \
             CASE WHEN $3 = 'ADMIN' THEN NULL ELSE repeat('a', 64) END, \
             CASE WHEN $3 = 'ADMIN' THEN NULL ELSE now() + interval '1 hour' END)",
        )
        .bind(id)
        .bind(format!("offboarding-{id}@example.com"))
        .bind(role)
        .bind(organization)
        .execute(&self.owner)
        .await
        .unwrap();
        id
    }

    /// An administrator, a member with every credential and support grant
    /// the command must end, and a pending join request to another company.
    async fn seed(&self, attached: bool) -> Seed {
        let organization = self.organization("FUEL_SUPPLIER").await;
        let other_organization = self.organization("FUEL_TRADER").await;
        let admin = self.user("ADMIN", None).await;
        let subject = self
            .user("SUPPLIER", attached.then_some(organization))
            .await;
        for _ in 0..2 {
            sqlx::query(
                "INSERT INTO refresh_sessions (id, user_id, family_id, jti_hash, device_id_hash, \
                 expires_at) VALUES ($1, $2, $3, $4, $5, now() + interval '7 days')",
            )
            .bind(Uuid::new_v4())
            .bind(subject)
            .bind(Uuid::new_v4())
            .bind(token_digest(&generate_token()))
            .bind(token_digest(&generate_token()))
            .execute(&self.owner)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at, created_at) \
             VALUES ($1, $2, now() + interval '1 hour', now())",
        )
        .bind(subject)
        .bind(token_digest(&generate_token()))
        .execute(&self.owner)
        .await
        .unwrap();
        let join = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organization_join_requests (id, user_id, organization_id, status, \
             created_at) VALUES ($1, $2, $3, 'PENDING', now())",
        )
        .bind(join)
        .bind(subject)
        .bind(other_organization)
        .execute(&self.owner)
        .await
        .unwrap();
        let context = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO market_support_contexts (id, actor_user_id, organization_id, \
             accountable_user_id, support_reference, expires_at) \
             VALUES ($1, $2, $3, $4, 'SUPPORT-1', now() + interval '1 hour')",
        )
        .bind(context)
        .bind(admin)
        .bind(organization)
        .bind(subject)
        .execute(&self.owner)
        .await
        .unwrap();
        sqlx::query(include_str!(
            "fixtures/offboarding_command_support_authorization.sql"
        ))
        .bind(Uuid::new_v4())
        .bind(organization)
        .bind(subject)
        .bind(PRODUCT)
        .bind(DELIVERY_POINT)
        .bind(admin)
        .bind(context)
        .execute(&self.owner)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO staff_capability_assignments (id, user_id, capability, reason, \
             granted_by_user_id) VALUES ($1, $2, 'MARKET_SUPPORT_LISTINGS', 'synthetic', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(subject)
        .bind(admin)
        .execute(&self.owner)
        .await
        .unwrap();
        Seed {
            organization,
            other_organization,
            admin,
            subject,
            join,
        }
    }

    /// Every row the command may change, read by the owner.
    async fn snapshot(&self, subject: Uuid) -> Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_object(\
             'user', (SELECT to_jsonb(u) FROM users u WHERE id = $1),\
             'sessions', (SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM refresh_sessions s WHERE user_id = $1),\
             'reset', (SELECT jsonb_agg(to_jsonb(r)) FROM password_reset_tokens r WHERE user_id = $1),\
             'joins', (SELECT jsonb_agg(to_jsonb(j) ORDER BY id) FROM organization_join_requests j WHERE user_id = $1),\
             'contexts', (SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM market_support_contexts c \
                          WHERE actor_user_id = $1 OR accountable_user_id = $1),\
             'authorizations', (SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM market_support_authorizations a \
                                WHERE accountable_user_id = $1),\
             'capabilities', (SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM staff_capability_assignments c \
                              WHERE user_id = $1),\
             'transitions', (SELECT count(*) FROM user_status_transitions WHERE user_id = $1),\
             'audit', (SELECT count(*) FROM audit_logs WHERE resource_id = $1::text),\
             'records', (SELECT count(*) FROM individual_offboardings WHERE subject_user_id = $1),\
             'outbox', (SELECT count(*) FROM market_event_outbox WHERE aggregate_id = $1::text),\
             'hook', (SELECT count(*) FROM offboarding_test_hook_calls WHERE subject_id = $1))",
        )
        .bind(subject)
        .fetch_one(&self.owner)
        .await
        .unwrap()
    }

    fn token(&self, user: Uuid) -> String {
        let now = Utc::now();
        let claims = json!({
            "sub": user, "iat": now.timestamp() - 1, "exp": now.timestamp() + 600,
            "type": "access", "iss": ISSUER, "aud": AUDIENCE,
        });
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    fn access(&self, user: Uuid) -> VerifiedAccess {
        verify_access(
            &self.token(user),
            &VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into()),
            Utc::now(),
        )
        .unwrap()
    }

    async fn send(
        &self,
        router: &Router,
        method: &str,
        path: &str,
        caller: Option<Uuid>,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(caller) = caller {
            request = request.header("authorization", format!("Bearer {}", self.token(caller)));
        }
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let response = router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn offboard(
        &self,
        caller: Option<Uuid>,
        subject: impl std::fmt::Display,
        expected: Option<Uuid>,
    ) -> (StatusCode, Value) {
        self.send(
            &self.router,
            "POST",
            &format!("/api/auth/admin/users/{subject}/offboard"),
            caller,
            Some(body(expected)),
            &[],
        )
        .await
    }

    async fn preview(
        &self,
        caller: Option<Uuid>,
        subject: impl std::fmt::Display,
    ) -> (StatusCode, Value) {
        self.send(
            &self.router,
            "GET",
            &format!("/api/auth/admin/users/{subject}/offboarding-preview"),
            caller,
            None,
            &[],
        )
        .await
    }

    async fn command(
        &self,
        subject: Uuid,
        expected: Option<Uuid>,
    ) -> OffboardAccount<Arc<RecordingHook>, NoEvents> {
        OffboardAccount {
            subject_id: subject,
            request: OffboardRequest::parse(&serde_json::to_vec(&body(expected)).unwrap()).unwrap(),
            unusable_hash: UnusablePasswordHash::prepare().await.unwrap(),
            audit: AuditContext::default(),
            market: Arc::new(RecordingHook),
            events: NoEvents,
        }
    }

    /// Wait until a runtime-role backend other than `holder` waits on a lock.
    /// The observer is the runtime role too: PostgreSQL shows another
    /// role's wait state only to privileged roles.
    async fn observe_lock_wait(&self, holder: &mut PgConnection) {
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        let started = Instant::now();
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE usename = $1 AND pid <> $2 \
                 AND wait_event_type = 'Lock' AND $2 = ANY(pg_blocking_pids(pid))",
            )
            .bind(&self.runtime_role)
            .bind(holder_pid)
            .fetch_one(&self.runtime)
            .await
            .unwrap();
            if waiting > 0 {
                return;
            }
            assert!(
                started.elapsed() < WAIT_OBSERVATION,
                "the command never waited for the held row lock"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A runtime-role connection that waits for locks longer than any
    /// command step takes.
    async fn concurrent(&self) -> PgConnection {
        let mut connection = PgConnection::connect(&disposable("RUST_TEST_DATABASE_URL"))
            .await
            .unwrap();
        sqlx::query("SELECT set_config('lock_timeout', '10s', false)")
            .execute(&mut connection)
            .await
            .unwrap();
        connection
    }

    async fn arm(&self, table: &str) {
        sqlx::query("INSERT INTO offboarding_test_failpoints VALUES ($1)")
            .bind(table)
            .execute(&self.owner)
            .await
            .unwrap();
    }

    async fn disarm(&self) {
        sqlx::query("DELETE FROM offboarding_test_failpoints")
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

fn body(expected: Option<Uuid>) -> Value {
    json!({
        "reason_code": "EMPLOYMENT_ENDED",
        "evidence_case_ref": CASE_REFERENCE,
        "urgent": true,
        "expected_organization_id": expected,
    })
}

fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap_or_default()
}

/// Records its call and what the transaction shows at that point. A subject
/// listed in `offboarding_test_hook_refusals` is refused as over the bound.
struct RecordingHook;

impl OffboardingMarketHook for RecordingHook {
    fn invalidate_user<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        scope: &'a UserMarketScope,
    ) -> CommandFuture<'a, MarketInvalidationSummary, AttemptError> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT status::text AS status, offboarded_at IS NOT NULL AS marked, \
                 organization_id FROM users WHERE id = $1",
            )
            .bind(scope.subject_id)
            .fetch_one(&mut *conn)
            .await?;
            sqlx::query("INSERT INTO offboarding_test_hook_calls VALUES ($1, $2, $3, $4, $5)")
                .bind(scope.subject_id)
                .bind(scope.actor_id)
                .bind(row.get::<String, _>("status"))
                .bind(row.get::<bool, _>("marked"))
                .bind(row.get::<Option<Uuid>, _>("organization_id"))
                .execute(&mut *conn)
                .await?;
            let refuse: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM offboarding_test_hook_refusals WHERE subject_id = $1)",
            )
            .bind(scope.subject_id)
            .fetch_one(&mut *conn)
            .await?;
            if refuse {
                return Err(refusal(
                    StatusCode::CONFLICT,
                    "MARKET_INVALIDATION_LIMIT",
                    "Market invalidation exceeds the transaction bound.",
                )
                .into());
            }
            Ok(MarketInvalidationSummary {
                withdrawn_orders: vec![Uuid::from_u128(1), Uuid::from_u128(2)],
                cancelled_trades: vec![Uuid::from_u128(3)],
                outbox_ids: vec![],
                released_quantity_mt: "4.500".parse().unwrap(),
            })
        })
    }
}

/// Direct commands do not need the admission event producer.
struct NoEvents;

impl AdmissionEvents for NoEvents {
    fn visible<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async { Ok(false) })
    }
    fn append<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _user: Uuid,
        _organization: Uuid,
        _public: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async { Ok(()) })
    }
}

async fn unchanged_after(harness: &Harness, subject: Uuid, before: &Value) {
    let after = harness.snapshot(subject).await;
    assert_eq!(
        &after, before,
        "the refused or failed command changed state"
    );
}

// ---------------------------------------------------------------------------
// Cases.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn commit_applies_every_account_change() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let before: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&harness.owner)
        .await
        .unwrap();
    let old_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(seed.subject)
        .fetch_one(&harness.owner)
        .await
        .unwrap();

    let (status, response) = harness
        .offboard(Some(seed.admin), seed.subject, Some(seed.organization))
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let operation: Uuid = response["operation_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(response["user_id"], json!(seed.subject));
    assert_eq!(response["former_organization_id"], json!(seed.organization));
    assert_eq!(response["status"], "REJECTED");
    assert!(response["offboarded_at"].is_string());
    assert_eq!(
        response.as_object().unwrap().len(),
        5,
        "no other field is returned: {response}"
    );

    let user = sqlx::query(
        "SELECT status::text AS status, organization_id, offboarded_at, kyc_status::text AS kyc, \
         kyc_organization_id, password_hash, password_changed_at, authentication_revoked_at, \
         password_reset_token_hash, pending_approval_email_payload IS NULL AS mail_cleared \
         FROM users WHERE id = $1",
    )
    .bind(seed.subject)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(user.get::<String, _>("status"), "REJECTED");
    assert_eq!(user.get::<Option<Uuid>, _>("organization_id"), None);
    // The marker is the database clock after the locks: not before the
    // request, and before the credential cutoff, which is read later and adds
    // the skew margin.
    let marked: DateTime<Utc> = user
        .get::<Option<DateTime<Utc>>, _>("offboarded_at")
        .unwrap();
    assert!(marked > before, "the marker follows the request");
    let password_cutoff: DateTime<Utc> = user
        .get::<Option<DateTime<Utc>>, _>("password_changed_at")
        .unwrap();
    assert!(
        marked <= password_cutoff - TimeDelta::seconds(CUTOFF_SKEW_MARGIN_SECONDS),
        "the marker precedes the cutoff's clock reading"
    );
    let answered: DateTime<Utc> = response["offboarded_at"].as_str().unwrap().parse().unwrap();
    assert_eq!(answered, marked, "the response reports the stored marker");
    assert_eq!(user.get::<String, _>("kyc"), "PENDING");
    assert_eq!(user.get::<Option<Uuid>, _>("kyc_organization_id"), None);
    assert_ne!(user.get::<String, _>("password_hash"), old_hash);
    for cutoff in ["password_changed_at", "authentication_revoked_at"] {
        let value: DateTime<Utc> = user.get::<Option<DateTime<Utc>>, _>(cutoff).unwrap();
        assert!(value > before, "{cutoff} must follow the command");
    }
    assert_eq!(
        user.get::<Option<String>, _>("password_reset_token_hash"),
        None
    );
    assert!(user.get::<bool, _>("mail_cleared"));

    let counts = sqlx::query(
        "SELECT \
         (SELECT count(*) FROM refresh_sessions WHERE user_id = $1 AND NOT revoked) AS live,\
         (SELECT count(*) FROM password_reset_tokens WHERE user_id = $1) AS resets,\
         (SELECT status::text FROM organization_join_requests WHERE id = $2) AS join_status,\
         (SELECT reviewed_by FROM organization_join_requests WHERE id = $2) AS join_reviewer,\
         (SELECT count(*) FROM market_support_contexts WHERE accountable_user_id = $1 \
          AND status = 'ACTIVE') AS contexts,\
         (SELECT count(*) FROM market_support_authorizations WHERE accountable_user_id = $1 \
          AND status = 'ACTIVE') AS authorizations,\
         (SELECT count(*) FROM staff_capability_assignments WHERE user_id = $1 \
          AND revoked_at IS NULL) AS capabilities,\
         (SELECT count(*) FROM user_status_transitions WHERE user_id = $1 \
          AND from_status = 'APPROVED' AND to_status = 'REJECTED') AS transitions",
    )
    .bind(seed.subject)
    .bind(seed.join)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(counts.get::<i64, _>("live"), 0);
    assert_eq!(counts.get::<i64, _>("resets"), 0);
    assert_eq!(counts.get::<String, _>("join_status"), "REJECTED");
    assert_eq!(
        counts.get::<Option<Uuid>, _>("join_reviewer"),
        Some(seed.admin)
    );
    assert_eq!(counts.get::<i64, _>("contexts"), 0);
    assert_eq!(counts.get::<i64, _>("authorizations"), 0);
    assert_eq!(counts.get::<i64, _>("capabilities"), 0);
    assert_eq!(counts.get::<i64, _>("transitions"), 1);

    // The market hook ran once, after the account changes, on the same transaction.
    let calls = sqlx::query(
        "SELECT actor_id, observed_status, observed_marker, observed_organization \
         FROM offboarding_test_hook_calls WHERE subject_id = $1",
    )
    .bind(seed.subject)
    .fetch_all(&harness.owner)
    .await
    .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].get::<Uuid, _>("actor_id"), seed.admin);
    assert_eq!(calls[0].get::<String, _>("observed_status"), "REJECTED");
    assert!(calls[0].get::<bool, _>("observed_marker"));
    assert_eq!(
        calls[0].get::<Option<Uuid>, _>("observed_organization"),
        None
    );

    let record = sqlx::query(
        "SELECT subject_user_id, former_organization_id, actor_user_id, reason_code, \
         evidence_case_ref, urgent, occurred_at FROM individual_offboardings WHERE operation_id = $1",
    )
    .bind(operation)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(record.get::<Uuid, _>("subject_user_id"), seed.subject);
    assert_eq!(
        record.get::<Option<Uuid>, _>("former_organization_id"),
        Some(seed.organization)
    );
    assert_eq!(record.get::<Uuid, _>("actor_user_id"), seed.admin);
    assert_eq!(record.get::<String, _>("reason_code"), "EMPLOYMENT_ENDED");
    assert_eq!(record.get::<String, _>("evidence_case_ref"), CASE_REFERENCE);
    assert!(record.get::<bool, _>("urgent"));
    assert_eq!(record.get::<DateTime<Utc>, _>("occurred_at"), marked);
    let (audit_at, transition_at): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT (SELECT timestamp FROM audit_logs WHERE resource_id = $1::text \
                 AND action = 'admin.user_offboarded'),\
                (SELECT effective_at FROM user_status_transitions WHERE user_id = $1 \
                 AND to_status = 'REJECTED')",
    )
    .bind(seed.subject)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!((audit_at, transition_at), (marked, marked));

    // The admission outbox event names the former organization only.
    let events = sqlx::query(
        "SELECT event_type, aggregate_type, participant_org_ids::jsonb AS participants \
         FROM market_event_outbox WHERE aggregate_id = $1",
    )
    .bind(seed.subject.to_string())
    .fetch_all(&harness.owner)
    .await
    .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].get::<String, _>("event_type"),
        "market_admission_changed"
    );
    assert_eq!(events[0].get::<String, _>("aggregate_type"), "user");
    assert_eq!(
        events[0].get::<Value, _>("participants"),
        json!([seed.organization])
    );
    let kyc_audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE resource_id = $1 \
         AND action = 'kyc.membership_invalidated' AND changes->>'reason' = 'account_offboarded'",
    )
    .bind(seed.subject.to_string())
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(kyc_audit, 1);
    let _ = seed.other_organization;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn expected_null_allows_detached_target() {
    let harness = Harness::open().await;
    let detached = harness.seed(false).await;

    // A detached target needs an explicit null, not an organization.
    let before = harness.snapshot(detached.subject).await;
    let (status, response) = harness
        .offboard(
            Some(detached.admin),
            detached.subject,
            Some(detached.organization),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&response), "OFFBOARD_MEMBERSHIP_CHANGED");
    unchanged_after(&harness, detached.subject, &before).await;

    let (status, response) = harness
        .offboard(Some(detached.admin), detached.subject, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["former_organization_id"], Value::Null);
    let row = sqlx::query(
        "SELECT status::text AS status, offboarded_at IS NOT NULL AS marked, kyc_status::text AS kyc \
         FROM users WHERE id = $1",
    )
    .bind(detached.subject)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "REJECTED");
    assert!(row.get::<bool, _>("marked"));
    assert_eq!(row.get::<String, _>("kyc"), "PENDING");
    // No organization work: no review invalidation and no admission event.
    let (kyc_audit, outbox): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM audit_logs WHERE resource_id = $1 \
                 AND action = 'kyc.membership_invalidated'),\
                (SELECT count(*) FROM market_event_outbox WHERE aggregate_id = $1)",
    )
    .bind(detached.subject.to_string())
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!((kyc_audit, outbox), (0, 0));

    // A member needs its current organization, not null.
    let member = harness.seed(true).await;
    let before = harness.snapshot(member.subject).await;
    let (status, response) = harness
        .offboard(Some(member.admin), member.subject, None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&response), "OFFBOARD_MEMBERSHIP_CHANGED");
    unchanged_after(&harness, member.subject, &before).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn admin_only_rejects_self_and_admin_target() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let other_admin = harness.user("ADMIN", None).await;
    let colleague = harness.user("SUPPLIER", Some(seed.organization)).await;
    let before = harness.snapshot(seed.subject).await;
    let admin_before = harness.snapshot(other_admin).await;
    let self_before = harness.snapshot(seed.admin).await;
    let expected = Some(seed.organization);

    // Authentication comes first, before the identifier is read.
    for target in [seed.subject.to_string(), "not-a-uuid".to_owned()] {
        let (status, _) = harness.offboard(None, &target, expected).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = harness.preview(None, &target).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    // A member, even of the same company, has no authority, and learns
    // nothing about the target identifier.
    for target in [
        seed.subject.to_string(),
        Uuid::new_v4().to_string(),
        "x".into(),
    ] {
        let (status, response) = harness.offboard(Some(colleague), &target, expected).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&response), "ADMIN_REQUIRED");
        let (status, response) = harness.preview(Some(colleague), &target).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&response), "ADMIN_REQUIRED");
    }
    // No assisted customer context authorizes the operation.
    for (method, path, body) in [
        (
            "POST",
            format!("/api/auth/admin/users/{}/offboard", seed.subject),
            Some(body(expected)),
        ),
        (
            "GET",
            format!("/api/auth/admin/users/{}/offboarding-preview", seed.subject),
            None,
        ),
    ] {
        let (status, response) = harness
            .send(
                &harness.router,
                method,
                &path,
                Some(seed.admin),
                body,
                &[(
                    "x-verdaxis-market-support-context",
                    "4b0c3c8e-31d4-4bda-9c4f-6d8d2b0a6f11",
                )],
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&response), "MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
    }
    // An administrator cannot offboard itself or another administrator.
    for target in [seed.admin, other_admin] {
        let (status, response) = harness.offboard(Some(seed.admin), target, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&response), "ACCOUNT_NOT_MANAGEABLE");
        let (status, response) = harness.preview(Some(seed.admin), target).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(code(&response), "ACCOUNT_NOT_MANAGEABLE");
    }
    // Input is validated after authority: identifier, then body.
    let (status, response) = harness
        .offboard(Some(seed.admin), "not-a-uuid", expected)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(code(&response), "VALIDATION_FAILED");
    let mut free_text = body(expected);
    free_text["evidence_case_ref"] = json!("left after a dispute");
    let path = format!("/api/auth/admin/users/{}/offboard", seed.subject);
    let (status, _) = harness
        .send(
            &harness.router,
            "POST",
            &path,
            Some(seed.admin),
            Some(free_text),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, response) = harness
        .offboard(Some(seed.admin), Uuid::new_v4(), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&response), "USER_NOT_FOUND");

    unchanged_after(&harness, seed.subject, &before).await;
    unchanged_after(&harness, other_admin, &admin_before).await;
    unchanged_after(&harness, seed.admin, &self_before).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn actor_is_revalidated_under_lock() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let before = harness.snapshot(seed.subject).await;
    // The administrator loses the role while the command waits for its row.
    let mut holder = harness.concurrent().await;
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(seed.admin)
        .fetch_one(&mut holder)
        .await
        .unwrap();
    let command = {
        let access = harness.access(seed.admin);
        let command = harness.command(seed.subject, Some(seed.organization)).await;
        let pool = harness.runtime.clone();
        tokio::spawn(run_ordered_write(
            pool,
            Arc::new(Semaphore::new(4)),
            access,
            command,
        ))
    };
    harness.observe_lock_wait(&mut holder).await;
    sqlx::query("UPDATE users SET role = 'BUYER' WHERE id = $1")
        .bind(seed.admin)
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
    let error = command
        .await
        .unwrap()
        .expect_err("a demoted actor must be refused");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.detail["code"], "ADMIN_REQUIRED");
    unchanged_after(&harness, seed.subject, &before).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn stale_membership_conflicts() {
    let harness = Harness::open().await;

    // The administrator saw an organization the member no longer has.
    let seed = harness.seed(true).await;
    let before = harness.snapshot(seed.subject).await;
    let (status, response) = harness
        .offboard(
            Some(seed.admin),
            seed.subject,
            Some(seed.other_organization),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&response), "OFFBOARD_MEMBERSHIP_CHANGED");
    unchanged_after(&harness, seed.subject, &before).await;

    // The membership moves while the command waits for the member's row: the
    // command discovers again under its locks and refuses the stale request.
    let seed = harness.seed(true).await;
    let mut holder = harness.concurrent().await;
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(seed.subject)
        .fetch_one(&mut holder)
        .await
        .unwrap();
    let command = {
        let access = harness.access(seed.admin);
        let command = harness.command(seed.subject, Some(seed.organization)).await;
        tokio::spawn(run_ordered_write(
            harness.runtime.clone(),
            Arc::new(Semaphore::new(4)),
            access,
            command,
        ))
    };
    harness.observe_lock_wait(&mut holder).await;
    sqlx::query(
        "UPDATE users SET organization_id = $2, kyc_organization_id = NULL, \
                 kyc_status = 'PENDING' WHERE id = $1",
    )
    .bind(seed.subject)
    .bind(seed.other_organization)
    .execute(&mut holder)
    .await
    .unwrap();
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
    let before_answer = harness.snapshot(seed.subject).await;
    let error = command
        .await
        .unwrap()
        .expect_err("a moved member must not be offboarded");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.detail["code"], "OFFBOARD_MEMBERSHIP_CHANGED");
    unchanged_after(&harness, seed.subject, &before_answer).await;
    let organization: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM users WHERE id = $1")
            .bind(seed.subject)
            .fetch_one(&harness.owner)
            .await
            .unwrap();
    assert_eq!(organization, Some(seed.other_organization));
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn duplicate_returns_offboard_already_completed() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let (status, _) = harness
        .offboard(Some(seed.admin), seed.subject, Some(seed.organization))
        .await;
    assert_eq!(status, StatusCode::OK);
    let after_first = harness.snapshot(seed.subject).await;
    for expected in [Some(seed.organization), None] {
        let (status, response) = harness
            .offboard(Some(seed.admin), seed.subject, expected)
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(code(&response), "OFFBOARD_ALREADY_COMPLETED");
    }
    unchanged_after(&harness, seed.subject, &after_first).await;
    let records: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM individual_offboardings WHERE subject_user_id = $1",
    )
    .bind(seed.subject)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(records, 1);
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn failure_rolls_back_every_account_change() {
    let harness = Harness::open().await;
    // Each table the command writes, in the order it writes them. The failure
    // fires right after the first change to that table.
    let tables = [
        "audit_logs",
        "users",
        "user_status_transitions",
        "refresh_sessions",
        "password_reset_tokens",
        "organization_join_requests",
        "market_support_contexts",
        "market_support_authorizations",
        "staff_capability_assignments",
        "market_event_outbox",
        "individual_offboardings",
    ];
    for attached in [true, false] {
        for table in tables {
            let seed = harness.seed(attached).await;
            let expected = attached.then_some(seed.organization);
            let before = harness.snapshot(seed.subject).await;
            harness.arm(table).await;
            let (status, response) = harness
                .offboard(Some(seed.admin), seed.subject, expected)
                .await;
            harness.disarm().await;
            if !attached && table == "market_event_outbox" {
                // A detached target appends no admission event.
                assert_eq!(status, StatusCode::OK, "{table}: {response}");
                continue;
            }
            assert_eq!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "attached={attached} {table}: {response}"
            );
            unchanged_after(&harness, seed.subject, &before).await;
        }
    }
    // The market hook refuses after every account change.
    let seed = harness.seed(true).await;
    sqlx::query("INSERT INTO offboarding_test_hook_refusals VALUES ($1)")
        .bind(seed.subject)
        .execute(&harness.owner)
        .await
        .unwrap();
    let before = harness.snapshot(seed.subject).await;
    let (status, response) = harness
        .offboard(Some(seed.admin), seed.subject, Some(seed.organization))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&response), "MARKET_INVALIDATION_LIMIT");
    unchanged_after(&harness, seed.subject, &before).await;
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn audit_contains_only_opaque_case_reference() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let (status, response) = harness
        .offboard(Some(seed.admin), seed.subject, Some(seed.organization))
        .await;
    assert_eq!(status, StatusCode::OK);
    let operation = response["operation_id"].as_str().unwrap().to_owned();
    let rows = sqlx::query(
        "SELECT user_id, resource_type, changes::jsonb AS changes FROM audit_logs \
         WHERE resource_id = $1 AND action = 'admin.user_offboarded'",
    )
    .bind(seed.subject.to_string())
    .fetch_all(&harness.owner)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<Option<Uuid>, _>("user_id"), Some(seed.admin));
    assert_eq!(rows[0].get::<String, _>("resource_type"), "user");
    let changes: Value = rows[0].get("changes");
    let mut keys: Vec<&str> = changes
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "cancelled_trade_ids",
            "cancelled_trades",
            "evidence_case_ref",
            "join_requests_closed",
            "kyc_review_invalidated",
            "kyc_state_unrecognized",
            "market_events",
            "operation_id",
            "organization_id",
            "reason_code",
            "refresh_families_revoked",
            "released_quantity_mt",
            "reset_tokens_removed",
            "staff_capabilities_revoked",
            "status",
            "support_authorizations_revoked",
            "support_contexts_closed",
            "urgent",
            "withdrawn_order_ids",
            "withdrawn_orders",
        ]
    );
    assert_eq!(changes["operation_id"], json!(operation));
    assert_eq!(changes["evidence_case_ref"], CASE_REFERENCE);
    assert_eq!(changes["reason_code"], "EMPLOYMENT_ENDED");
    assert_eq!(changes["urgent"], true);
    assert_eq!(
        changes["status"],
        json!({"from": "APPROVED", "to": "REJECTED"})
    );
    assert_eq!(
        changes["organization_id"],
        json!({"from": seed.organization, "to": null})
    );
    assert_eq!(changes["refresh_families_revoked"], 2);
    assert_eq!(changes["reset_tokens_removed"], 1);
    assert_eq!(changes["join_requests_closed"], 1);
    assert_eq!(changes["support_contexts_closed"], 1);
    assert_eq!(changes["support_authorizations_revoked"], 1);
    assert_eq!(changes["staff_capabilities_revoked"], 1);
    assert_eq!(changes["withdrawn_orders"], 2);
    assert_eq!(changes["cancelled_trades"], 1);
    // The record links to the rows the market work changed and states the
    // tonnes it released.
    assert_eq!(
        changes["withdrawn_order_ids"],
        json!([Uuid::from_u128(1), Uuid::from_u128(2)])
    );
    assert_eq!(changes["cancelled_trade_ids"], json!([Uuid::from_u128(3)]));
    assert_eq!(changes["released_quantity_mt"], "4.5");
    // Only counts, identifiers and the opaque reference: every string value is
    // one of the known identifiers or codes, never an email, hash or secret.
    let email = format!("offboarding-{}@example.com", seed.subject);
    let text = changes.to_string();
    for forbidden in [email.as_str(), "synthetic-unused-hash", "$2b$", "@"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
    // The reference appears in this record and the history row, nowhere else.
    let elsewhere: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE changes::text LIKE '%' || $1 || '%' \
         AND NOT (resource_id = $2 AND action = 'admin.user_offboarded')",
    )
    .bind(CASE_REFERENCE)
    .bind(seed.subject.to_string())
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(elsewhere, 0);
    let outbox: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM market_event_outbox WHERE payload::text LIKE '%' || $1 || '%'",
    )
    .bind(CASE_REFERENCE)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(outbox, 0);
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn offboarding_lock_winner_blocks_reattachment() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let control = CompletionControl::new(CompletionPoint::BeforeCommit, false);
    let command = tokio::spawn(run_controlled_write(
        harness.runtime.clone(),
        Arc::new(Semaphore::new(4)),
        harness.access(seed.admin),
        harness.command(seed.subject, Some(seed.organization)).await,
        control.clone(),
    ));
    control.reached().await;
    // An approval or join that reaches the row now waits, then meets the
    // committed marker and the shared constraint.
    let mut writer = harness.concurrent().await;
    let organization = seed.organization;
    let subject = seed.subject;
    let restore = tokio::spawn(async move {
        let result =
            sqlx::query("UPDATE users SET status = 'APPROVED', organization_id = $2 WHERE id = $1")
                .bind(subject)
                .bind(organization)
                .execute(&mut writer)
                .await;
        (result, writer)
    });
    let mut observer = harness.concurrent().await;
    // The command's connection holds the row; wait until the writer queues.
    let started = Instant::now();
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE usename = $1 \
             AND wait_event_type = 'Lock' AND query LIKE 'UPDATE users SET status%'",
        )
        .bind(&harness.runtime_role)
        .fetch_one(&mut observer)
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        assert!(
            started.elapsed() < WAIT_OBSERVATION,
            "the writer never waited"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    control.release();
    command.await.unwrap().expect("the command commits first");
    let (result, _writer) = restore.await.unwrap();
    let error = result.expect_err("a marked account cannot be restored");
    let database = error.as_database_error().unwrap();
    assert_eq!(database.code().as_deref(), Some("23514"));
    let row =
        sqlx::query("SELECT status::text AS status, organization_id FROM users WHERE id = $1")
            .bind(seed.subject)
            .fetch_one(&harness.owner)
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>("status"), "REJECTED");
    assert_eq!(row.get::<Option<Uuid>, _>("organization_id"), None);
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn session_lock_winner_is_cut_off() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    // A sign-in holds the member's row and adds a refresh family while the
    // command waits behind it.
    let mut login = harness.concurrent().await;
    sqlx::query("BEGIN").execute(&mut login).await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(seed.subject)
        .fetch_one(&mut login)
        .await
        .unwrap();
    let command = {
        let access = harness.access(seed.admin);
        let command = harness.command(seed.subject, Some(seed.organization)).await;
        tokio::spawn(run_ordered_write(
            harness.runtime.clone(),
            Arc::new(Semaphore::new(4)),
            access,
            command,
        ))
    };
    harness.observe_lock_wait(&mut login).await;
    let family = Uuid::new_v4();
    let issued: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO refresh_sessions (id, user_id, family_id, jti_hash, device_id_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() + interval '7 days') RETURNING clock_timestamp()",
    )
    .bind(Uuid::new_v4())
    .bind(seed.subject)
    .bind(family)
    .bind(token_digest(&generate_token()))
    .bind(token_digest(&generate_token()))
    .fetch_one(&mut login)
    .await
    .unwrap();
    sqlx::query("COMMIT").execute(&mut login).await.unwrap();
    command
        .await
        .unwrap()
        .expect("the command commits after the sign-in");
    let (revoked, cutoff, marked): (bool, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT (SELECT revoked FROM refresh_sessions WHERE family_id = $2),\
                (SELECT authentication_revoked_at FROM users WHERE id = $1),\
                (SELECT offboarded_at FROM users WHERE id = $1)",
    )
    .bind(seed.subject)
    .bind(family)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert!(revoked, "the family issued during the wait must be revoked");
    assert!(
        cutoff > issued + TimeDelta::seconds(1),
        "the cutoff must follow a token issued during the wait"
    );
    assert!(
        marked > issued,
        "the recorded offboarding time follows the sign-in that won the lock"
    );
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn preview_reports_advisory_counts() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let counterparty = harness.organization("FUEL_BUYER").await;
    for status in ["OPEN", "PARTIALLY_FILLED", "FILLED"] {
        sqlx::query(include_str!("fixtures/offboarding_command_order.sql"))
            .bind(Uuid::new_v4())
            .bind(seed.organization)
            .bind(seed.subject)
            .bind(PRODUCT)
            .bind(DELIVERY_POINT)
            .bind(status)
            .execute(&harness.owner)
            .await
            .unwrap();
    }
    let mut pending = Vec::new();
    for (status, count) in [
        ("PENDING_CONFIRMATION", 2),
        ("CONFIRMED", 3),
        ("DELIVERED", 1),
        ("PAID", 4),
    ] {
        for _ in 0..count {
            let id = trade(
                &harness,
                counterparty,
                seed.organization,
                seed.subject,
                status,
            )
            .await;
            if status == "PENDING_CONFIRMATION" {
                pending.push(id);
            }
        }
    }
    // One of the two pending trades is frozen: the preview reports it as a
    // blocker next to the pending count.
    sqlx::query(
        "INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id,version) \
         VALUES($1,$2,'ACTIVE','COMPLIANCE_REVIEW','fixture review',$3,1)",
    )
    .bind(Uuid::new_v4())
    .bind(pending[0])
    .bind(seed.admin)
    .execute(&harness.owner)
    .await
    .unwrap();
    let before = harness.snapshot(seed.subject).await;
    let (status, preview) = harness.preview(Some(seed.admin), seed.subject).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["user_id"], json!(seed.subject));
    assert_eq!(
        preview["email"],
        format!("offboarding-{}@example.com", seed.subject)
    );
    assert_eq!(preview["status"], "APPROVED");
    assert_eq!(preview["organization_id"], json!(seed.organization));
    assert!(preview["organization_name"].is_string());
    assert_eq!(preview["offboarded_at"], Value::Null);
    assert_eq!(preview["open_owned_orders"], 2);
    assert_eq!(preview["pending_trades"], 2);
    assert_eq!(preview["frozen_pending_trades"], 1);
    assert_eq!(preview["confirmed_trades"], 3);
    assert_eq!(preview["delivered_trades"], 1);
    assert_eq!(preview["informational_counts_capped"], false);
    let mut keys: Vec<&str> = preview
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "confirmed_trades",
            "delivered_trades",
            "email",
            "frozen_pending_trades",
            "informational_counts_capped",
            "offboarded_at",
            "open_owned_orders",
            "organization_id",
            "organization_name",
            "pending_trades",
            "status",
            "user_id",
        ],
        "no case, KYC or credential field is exposed"
    );
    // The preview reads only.
    unchanged_after(&harness, seed.subject, &before).await;
    let (status, _) = harness.preview(Some(seed.admin), Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn preview_refuses_overflow() {
    let harness = Harness::open().await;
    let counterparty = harness.organization("FUEL_BUYER").await;

    // Confirmed and delivered trades are informational: above the bound the
    // count is capped and flagged, and the preview still answers.
    let seed = harness.seed(true).await;
    for status in ["CONFIRMED", "DELIVERED"] {
        for _ in 0..101 {
            trade(
                &harness,
                counterparty,
                seed.organization,
                seed.subject,
                status,
            )
            .await;
        }
    }
    let (status, preview) = harness.preview(Some(seed.admin), seed.subject).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["confirmed_trades"], 100);
    assert_eq!(preview["delivered_trades"], 100);
    assert_eq!(preview["informational_counts_capped"], true);

    // Pending trades are cancelled by the command: 100 answer, 101 refuse.
    let seed = harness.seed(true).await;
    for _ in 0..100 {
        trade(
            &harness,
            counterparty,
            seed.organization,
            seed.subject,
            "PENDING_CONFIRMATION",
        )
        .await;
    }
    let (status, preview) = harness.preview(Some(seed.admin), seed.subject).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["pending_trades"], 100);
    assert_eq!(preview["informational_counts_capped"], false);
    trade(
        &harness,
        counterparty,
        seed.organization,
        seed.subject,
        "PENDING_CONFIRMATION",
    )
    .await;
    let (status, response) = harness.preview(Some(seed.admin), seed.subject).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&response), "OFFBOARD_PREVIEW_LIMIT");

    // Open order remainders are withdrawn by the command: 101 refuse.
    let seed = harness.seed(true).await;
    for _ in 0..101 {
        order(&harness, seed.organization, seed.subject, "OPEN").await;
    }
    let (status, response) = harness.preview(Some(seed.admin), seed.subject).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&response), "OFFBOARD_PREVIEW_LIMIT");
}

/// One ten-tonne ask owned by the subject.
async fn order(harness: &Harness, organization: Uuid, owner: Uuid, status: &str) {
    sqlx::query(include_str!("fixtures/offboarding_command_order.sql"))
        .bind(Uuid::new_v4())
        .bind(organization)
        .bind(owner)
        .bind(PRODUCT)
        .bind(DELIVERY_POINT)
        .bind(status)
        .execute(&harness.owner)
        .await
        .unwrap();
}

/// One trade where the subject is the recorded seller.
async fn trade(
    harness: &Harness,
    buyer: Uuid,
    seller: Uuid,
    seller_user: Uuid,
    status: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(include_str!("fixtures/offboarding_command_trade.sql"))
        .bind(id)
        .bind(buyer)
        .bind(seller)
        .bind(seller_user)
        .bind(status)
        .bind(PRODUCT)
        .bind(DELIVERY_POINT)
        .execute(&harness.owner)
        .await
        .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn unrecognized_kyc_state_does_not_block_offboarding() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    sqlx::query("UPDATE users SET kyc_status = 'GARBLED' WHERE id = $1")
        .bind(seed.subject)
        .execute(&harness.owner)
        .await
        .unwrap();
    let (status, response) = harness
        .offboard(Some(seed.admin), seed.subject, Some(seed.organization))
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let row = sqlx::query(
        "SELECT status::text AS status, offboarded_at IS NOT NULL AS marked, \
         organization_id, kyc_status::text AS kyc FROM users WHERE id = $1",
    )
    .bind(seed.subject)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "REJECTED");
    assert!(row.get::<bool, _>("marked"));
    assert_eq!(row.get::<Option<Uuid>, _>("organization_id"), None);
    // The unrecognized value is kept for an operator, and the audit says so.
    assert_eq!(row.get::<String, _>("kyc"), "GARBLED");
    let changes: Value = sqlx::query_scalar(
        "SELECT changes::jsonb FROM audit_logs WHERE resource_id = $1::text \
         AND action = 'admin.user_offboarded'",
    )
    .bind(seed.subject)
    .fetch_one(&harness.owner)
    .await
    .unwrap();
    assert_eq!(changes["kyc_state_unrecognized"], true);
    assert_eq!(changes["kyc_review_invalidated"], false);
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn production_composition_does_not_route_offboarding() {
    let harness = Harness::open().await;
    let seed = harness.seed(true).await;
    let before = harness.snapshot(seed.subject).await;
    let serving = verdaxis_read::router(harness.state.clone());
    let (status, _) = harness
        .send(
            &serving,
            "POST",
            &format!("/api/auth/admin/users/{}/offboard", seed.subject),
            Some(seed.admin),
            Some(body(Some(seed.organization))),
            &[("host", "localhost")],
        )
        .await;
    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "{status}"
    );
    let (status, _) = harness
        .send(
            &serving,
            "GET",
            &format!("/api/auth/admin/users/{}/offboarding-preview", seed.subject),
            Some(seed.admin),
            None,
            &[("host", "localhost")],
        )
        .await;
    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "{status}"
    );
    unchanged_after(&harness, seed.subject, &before).await;
    let _ = IGNORE;
}

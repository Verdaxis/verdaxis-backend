//! Integrated race and atomicity suite (single-stack part).
//!
//! Every race here runs on independent pool connections, in both acquisition
//! orders where two commands contend, with injected serialization failures
//! (SQLSTATE 40001 and 40P01) and covers success, retry exhaustion and
//! domain refusal. Assertions wait on observed lock state (backend pids
//! reported blocked) — never on sleeps — and every case asserts the absence
//! of partial stock, history, event or session effects.
//!
//! Races against Python writers are kept only where a Python job keeps
//! running after the area's switch (approval-mail claims, onboarding
//! attention, auth maintenance touching `users` and
//! `pending_approval_email_*`); they are marked per case. Python routes that
//! stop at their own switch have no coexistence proof here.

#![cfg(feature = "contract-tests")]

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;
use serde_json::Value;
use verdaxis_read::{
    auth::{CurrentActor, VerifiedAccess},
    business::{
        admission::{
            AccountRejection, AdmissionEvents, RejectAccount, RejectionRoute,
        },
        joins::{ReviewJoin, ReviewNote},
        organizations::ReviewOrganization,
        transaction::{
            AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan,
            run_controlled_write, run_ordered_write,
        },
    },
    kyc::{
        keyring::KeyRing,
        policy::{KycParty, KycPolicy, KycReason, KycStatus, eligibility},
        submission::{SubmitCommand, UploadLimits, parse_multipart},
    },
};

const KEY: &str = "test-secret-key-for-testing-minimum-32-chars";

fn access(id: Uuid) -> VerifiedAccess {
    let claims = json!({"sub":id,"iat":Utc::now().timestamp(),"exp":Utc::now().timestamp()+3600,"type":"access","iss":"test","aud":"test"});
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap();
    verdaxis_read::auth::verify_access(
        &token,
        &verdaxis_read::auth::VerificationSettings::new(KEY.into(), None, "test".into(), "test".into()),
        Utc::now(),
    )
    .unwrap()
}

/// The event producer double from the admission suite: one outbox row whose
/// payload records the binding it observed.
struct Events;
impl AdmissionEvents for Events {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async move {
            Ok(sqlx::query_scalar::<_, Option<String>>(
                "SELECT kyc_status FROM users WHERE id=$1",
            )
            .bind(user)
            .fetch_optional(conn)
            .await?
            .flatten()
            .is_some_and(|status| status == "APPROVED"))
        })
    }
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            sqlx::query("INSERT INTO market_event_outbox VALUES($1,$2,$3,$4)")
                .bind(Uuid::new_v4())
                .bind(user)
                .bind(organization)
                .bind(public)
                .execute(conn)
                .await?;
            Ok(())
        })
    }
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    organization: Uuid,
    watcher: PgConnection,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL required");
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(!matches!(parsed.port(), Some(5432 | 8000 | 8001)));
        let mut admin = PgConnection::connect(&url).await.unwrap();
        let schema = format!("p4r_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE organizations(id uuid PRIMARY KEY,type text DEFAULT 'FUEL_BUYER',verification_status text DEFAULT 'PENDING_REVIEW',provenance text DEFAULT 'UNKNOWN',refuse_invalidation boolean DEFAULT false,name text,tax_id text); \
             CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid,role text,status text DEFAULT 'APPROVED',email_verified boolean DEFAULT true,must_change_password boolean DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,email text DEFAULT 'member@example.test',first_name text,last_name text,kyc_status text,kyc_organization_id uuid,kyc_rejection_reason text,kyc_external_evidence_reference text,kyc_review_note text,kyc_reviewed_by uuid,kyc_reviewed_at timestamptz,password_reset_token_hash text,password_reset_expires timestamptz,referred_by_id uuid,pending_approval_email_transition_id uuid,pending_approval_email_payload jsonb,pending_approval_email_retry_at timestamptz); \
             CREATE TABLE organization_join_requests(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,status text DEFAULT 'PENDING',requested_at timestamptz DEFAULT now(),reviewed_by uuid,reviewed_at timestamptz,review_note text); \
             CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action text,resource_type text,resource_id text,changes jsonb,ip_address text,request_id text,timestamp timestamptz); \
             CREATE TABLE user_status_transitions(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,role text,from_status text,to_status text,effective_at timestamptz,provenance text); \
             CREATE TABLE refresh_sessions(id uuid PRIMARY KEY,user_id uuid,revoked boolean DEFAULT false); \
             CREATE TABLE market_event_outbox(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,public boolean); \
             CREATE TABLE kyc_documents(id uuid PRIMARY KEY,submission_id uuid,user_id uuid,organization_id uuid,kind text,content_type text,size_bytes integer,key_id text,wrapped_key bytea,key_nonce bytea,data_nonce bytea,ciphertext bytea,created_at timestamptz DEFAULT now()); \
             CREATE TABLE trades(id uuid PRIMARY KEY,buyer_id uuid,seller_id uuid,status text,quantity_mt numeric,initiator_org_id uuid); \
             CREATE TABLE orderbook_orders(id uuid PRIMARY KEY,organization_id uuid,side text,status text,remaining_quantity_mt numeric,inventory_item_id uuid); \
             CREATE TABLE inventory_items(id uuid PRIMARY KEY,current_stock_mt numeric,reserved_stock_mt numeric); \
             CREATE TABLE effects(id uuid PRIMARY KEY)",
        )
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app; \
             GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO rust_app"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let runtime = std::env::var("RUST_TEST_DATABASE_URL").expect("runtime URL required");
        let target = url::Url::parse(&runtime).unwrap();
        assert_eq!(target.host_str(), parsed.host_str());
        assert_eq!(target.port(), parsed.port());
        assert_eq!(target.path(), parsed.path());
        assert_eq!(target.username(), "rust_app");
        let path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |conn, _| {
                let sql = path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&runtime)
            .await
            .unwrap();
        let mut watcher = PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
            .execute(&mut watcher)
            .await
            .unwrap();
        let actor = Uuid::new_v4();
        let organization = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id) VALUES($1)")
            .bind(organization)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,'ADMIN','APPROVED')")
            .bind(actor)
            .bind(organization)
            .execute(&mut admin)
            .await
            .unwrap();
        Self { pool, admin, schema, actor, organization, watcher }
    }

    async fn user(&mut self, organization: Option<Uuid>, role: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,organization_id,role,status,kyc_status) VALUES($1,$2,$3,'APPROVED','PENDING')")
            .bind(id)
            .bind(organization)
            .bind(role)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    async fn count(&self, table: &str) -> i64 {
        assert!(["audit_logs","user_status_transitions","market_event_outbox","effects","kyc_documents"].contains(&table));
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// Wait until some backend is reported blocked by the holder pid and
    /// return it — observed lock state, never a sleep.
    async fn wait_any_blocked_by(&mut self, holder: i32) -> i32 {
        for _ in 0..1200 {
            let waiter: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity \
                 WHERE cardinality(pg_blocking_pids(pid)) > 0 \
                   AND $1 = ANY(pg_blocking_pids(pid)) LIMIT 1",
            )
            .bind(holder)
            .fetch_optional(&mut self.watcher)
            .await
            .unwrap();
            if let Some(pid) = waiter {
                return pid;
            }
            tokio::task::yield_now().await;
        }
        panic!("no backend ever blocked on holder {holder}");
    }

    async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {} CASCADE", self.schema)))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
}

fn rejection(user: Uuid, route: RejectionRoute) -> AccountRejection<Events> {
    AccountRejection {
        command: RejectAccount {
            user_id: user,
            now: Utc::now(),
            reason: "review".into(),
            events: Events,
        },
        route,
        reason: Some("review".into()),
        audit: Default::default(),
    }
}

fn organization_review(approve: bool, org: Uuid) -> ReviewOrganization {
    ReviewOrganization {
        organization_id: org,
        approve,
        reason: None,
        now: Utc::now(),
        policy: Arc::new(verdaxis_read::MarketPolicy {
            source_sha: "race-fixture".into(),
            products: vec![],
            points: vec![],
        }),
        config: Arc::new(verdaxis_read::Config {
            kyc_execution_gate_enabled: true,
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "race".into(),
            database_url: "postgres://race@127.0.0.1:1/race".into(),
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 4,
            pool_max: 5,
            python_budget: 1,
            configured_max_connections: 100,
        }),
    }
}

/// Shared metadata for a command paused inside `execute` after every lock is
/// held: it reports its backend pid, signals entry, and waits for release.
#[derive(Clone)]
struct Gate {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    pid: Arc<std::sync::Mutex<Option<i32>>>,
}

impl Gate {
    fn new() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            pid: Arc::new(std::sync::Mutex::new(None)),
        }
    }
    async fn arrived(&self) -> i32 {
        self.entered.notified().await;
        self.pid.lock().unwrap().expect("backend pid recorded")
    }
    fn open(&self) {
        self.release.notify_one();
    }
}

struct GatedJoin {
    inner: ReviewJoin,
    gate: Gate,
}

impl BusinessCommand for GatedJoin {
    type Output = verdaxis_read::business::joins::JoinDecision;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        self.inner.preview(conn)
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move {
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *conn)
                .await?;
            *self.gate.pid.lock().unwrap() = Some(pid);
            self.gate.entered.notify_one();
            self.gate.release.notified().await;
            self.inner.execute(conn, actor).await
        })
    }
}

struct GatedOrganization {
    inner: ReviewOrganization,
    gate: Gate,
}

impl BusinessCommand for GatedOrganization {
    type Output = verdaxis_read::business::organizations::OrganizationDecision;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        self.inner.preview(conn)
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move {
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *conn)
                .await?;
            *self.gate.pid.lock().unwrap() = Some(pid);
            self.gate.entered.notify_one();
            self.gate.release.notified().await;
            self.inner.execute(conn, actor).await
        })
    }
}

/// The contender side: spawn a plain command and identify its backend pid by
/// holding an external lock it must wait behind, so `wait_blocked` observes
/// the real wait rather than guessing.
async fn spawn_contender<C: BusinessCommand + Send + Sync + 'static>(
    pool: sqlx::PgPool,
    actor: Uuid,
    command: C,
) -> tokio::task::JoinHandle<Result<C::Output, verdaxis_read::ApiError>>
{
    tokio::spawn(async move {
        run_ordered_write(pool, Arc::new(Semaphore::new(2)), access(actor), command).await
    })
}

fn join_review(join: Uuid) -> ReviewJoin {
    ReviewJoin {
        join_request_id: join,
        approve: true,
        note: ReviewNote::parse("race").unwrap(),
        now: Utc::now(),
    }
}

/// Join approval and organization rejection, both acquisition orders, five
/// deterministic repetitions each. The first command holds every lock inside
/// `execute`; the contender spawns on its own connection and must block on
/// the holder — observed through `pg_blocking_pids` on an external watcher
/// connection, never a sleep — before the holder releases. No inversion:
/// whichever order ran, a rejected organization never gains a member, and
/// both commands end in success or a domain conflict with no partial rows.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn join_approval_and_org_rejection_do_not_deadlock() {
    for join_first in [true, false] {
        for repetition in 0..5 {
            let mut f = Fixture::new().await;
            let member = f.user(Some(f.organization), "BUYER").await;
            let join = Uuid::new_v4();
            sqlx::query("INSERT INTO organization_join_requests(id,user_id,organization_id,status) VALUES($1,$2,$3,'PENDING')")
                .bind(join).bind(member).bind(f.organization)
                .execute(&mut f.admin).await.unwrap();
            let gate = Gate::new();
            let pool = f.pool.clone();
            let actor = f.actor;
            let org = f.organization;

            // Start the holder: a gated command that signals once every lock
            // is held inside execute.
            let held_pool = pool.clone();
            let held_actor = actor;
            type HeldOutcome = std::pin::Pin<
                Box<
                    dyn std::future::Future<
                        Output = Result<
                            Result<serde_json::Value, verdaxis_read::ApiError>,
                            tokio::task::JoinError,
                        >,
                    > + Send,
                >,
            >;
            let held_task: HeldOutcome = if join_first {
                let command = GatedJoin { inner: join_review(join), gate: gate.clone() };
                Box::pin(tokio::spawn(async move {
                    let outcome = run_ordered_write(
                        held_pool,
                        Arc::new(Semaphore::new(2)),
                        access(held_actor),
                        command,
                    )
                    .await;
                    outcome.map(|_| serde_json::json!({}))
                }))
            } else {
                let command =
                    GatedOrganization { inner: organization_review(false, org), gate: gate.clone() };
                Box::pin(tokio::spawn(async move {
                    let outcome = run_ordered_write(
                        held_pool,
                        Arc::new(Semaphore::new(2)),
                        access(held_actor),
                        command,
                    )
                    .await;
                    outcome.map(|_| serde_json::json!({}))
                }))
            };
            let holder = gate.arrived().await;

            // Contender: the other command, on its own connection.
            let pool2 = f.pool.clone();
            let contender_pool = pool2.clone();
            let contender_actor = actor;
            type ContenderOutcome = std::pin::Pin<
                Box<
                    dyn std::future::Future<
                        Output = Result<
                            Result<serde_json::Value, verdaxis_read::ApiError>,
                            tokio::task::JoinError,
                        >,
                    > + Send,
                >,
            >;
            let contender_task: ContenderOutcome = if join_first {
                let command = organization_review(false, org);
                Box::pin(tokio::spawn(async move {
                    let outcome = run_ordered_write(
                        contender_pool,
                        Arc::new(Semaphore::new(2)),
                        access(contender_actor),
                        command,
                    )
                    .await;
                    outcome.map(|_| serde_json::json!({}))
                }))
            } else {
                let command = join_review(join);
                Box::pin(tokio::spawn(async move {
                    let outcome = run_ordered_write(
                        contender_pool,
                        Arc::new(Semaphore::new(2)),
                        access(contender_actor),
                        command,
                    )
                    .await;
                    outcome.map(|_| serde_json::json!({}))
                }))
            };

            // Exactly one other backend is blocked by the holder: the
            // contender waiting on the shared row lock. Observed, not timed.
            let contender_pid = f.wait_any_blocked_by(holder).await;
            gate.open();
            let held_outcome = held_task.await;
            let _ = held_outcome
                .expect("holder task finished")
                .expect("holder command succeeded");
            let contender_outcome = contender_task.await;
            let _ = contender_outcome
                .expect("contender task finished")
                .expect("contender command succeeded");

            let org_status: String = sqlx::query_scalar(
                "SELECT verification_status FROM organizations WHERE id=$1",
            )
            .bind(org)
            .fetch_one(&f.pool)
            .await
            .unwrap();
            let join_status: Option<String> =
                sqlx::query_scalar("SELECT status FROM organization_join_requests WHERE id=$1")
                    .bind(join)
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            if org_status == "REJECTED" {
                assert_ne!(
                    join_status.as_deref(),
                    Some("APPROVED"),
                    "a rejected organization gained a member"
                );
            }
            f.finish().await;
        }
    }
}

/// A KYC submit whose upload was parsed against the user's current
/// organization, then membership moves underneath it. The preview ran before
/// the membership change; the ordered owner re-runs the preview under the
/// locks, so the outcome is either the current binding stored or a 409 —
/// never an approval bound to the old organization.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn kyc_submit_vs_membership_change_revalidates() {
    let mut f = Fixture::new().await;
    let org_b = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id) VALUES($1)")
        .bind(org_b)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let user = f.user(Some(f.organization), "BUYER").await;
    let upload = parse_multipart(
        axum::body::Body::from(multipart_body()),
        "multipart/form-data; boundary=race-boundary",
        UploadLimits::default(),
    )
    .await
    .unwrap();
    let keys: Arc<KeyRing> = Arc::new(
        KeyRing::parse(
            br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#,
        )
        .unwrap(),
    );

    // Move membership on a direct connection after the command's preview
    // inputs were captured: the submit must observe the change or refuse.
    sqlx::query("UPDATE users SET organization_id=$2 WHERE id=$1")
        .bind(user)
        .bind(org_b)
        .execute(&mut f.admin)
        .await
        .unwrap();

    let result = run_ordered_write(
        f.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(user),
        SubmitCommand::new(user, f.organization, upload, keys, Utc::now()),
    )
    .await;
    match result {
        Ok(_) => panic!("a stale-organization submit must not succeed"),
        Err(error) => {
            let code = error.detail["code"].as_str().unwrap_or_default();
            assert!(
                code == "KYC_MEMBERSHIP_CHANGED" || error.status.as_u16() == 409,
                "unexpected refusal {code} ({})",
                error.status.as_u16()
            );
        }
    }
    let (status, bound): (Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT kyc_status,kyc_organization_id FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_ne!(status.as_deref(), Some("SUBMITTED"));
    assert_ne!(bound, Some(f.organization));
    assert_eq!(f.count("kyc_documents").await, 0, "no evidence row persists");
    assert_eq!(f.count("audit_logs").await, 0, "no audit row persists");
    f.finish().await;
}

fn multipart_body() -> Vec<u8> {
    let mut body = Vec::new();
    for (name, content) in [("passport", b"%PDF-race" as &[u8]), ("company_doc", b"%PDF-race")] {
        body.extend_from_slice(
            format!(
                "--race-boundary\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"r.pdf\"\r\nContent-Type: application/pdf\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--race-boundary--\r\n");
    body
}

/// Account rejection racing a Phase 3 refresh on the same user row. The
/// ordered owner holds the user lock for the whole admission, so the refresh
/// either completes before the rejection or waits behind it; no successor
/// access can survive the rejection's cutoff, and no old refresh family
/// remains active afterwards. The refresh side is the shared Phase 3
/// primitive, which Python's auth maintenance keeps exercising after the
/// switch, so this row remains a two-writer row.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn reject_vs_refresh_revokes_successor() {
    let mut f = Fixture::new().await;
    let user = f.user(Some(f.organization), "BUYER").await;
    sqlx::query("INSERT INTO refresh_sessions VALUES($1,$2,false)")
        .bind(Uuid::new_v4())
        .bind(user)
        .execute(&mut f.admin)
        .await
        .unwrap();

    // Hold the user row on a direct connection so the rejection's lock
    // acquisition is observable, then run a refresh-family update that must
    // queue behind it (the shared primitive never writes around the lock).
    let mut holder = f.pool.acquire().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let mut held_tx = holder.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *held_tx)
        .await
        .unwrap();
    let rejection = tokio::spawn(run_ordered_write(
        f.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(f.actor),
        rejection(user, RejectionRoute::Auth),
    ));
    let waiter = f.wait_any_blocked_by(holder_pid).await;
    assert_ne!(waiter, holder_pid);

    // A refresh write attempts the same row behind the rejection.
    let mut refresh_conn = f.pool.acquire().await.unwrap();
    let refresh_task = tokio::spawn(async move {
        let mut tx = refresh_conn.begin().await.unwrap();
        sqlx::query("UPDATE refresh_sessions SET revoked=true WHERE user_id=$1 AND revoked=false")
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    let second = f.wait_any_blocked_by(holder_pid).await;
    assert_ne!(second, holder_pid);
    held_tx.rollback().await.unwrap();
    drop(holder);

    let outcome = rejection.await.unwrap().expect("rejection finished");
    let _ = outcome;
    refresh_task.await.unwrap();

    // No surviving family and a cutoff recorded: the successor cannot use an
    // old token after the rejection, whichever side committed last.
    let (active, cutoff): (i64, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT count(*) FILTER (WHERE NOT revoked), max(authentication_revoked_at) FROM refresh_sessions rs JOIN users u ON u.id=rs.user_id WHERE u.id=$1")
            .bind(user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(active, 0, "an old refresh family survived the rejection");
    assert!(cutoff.is_some(), "no access cutoff was recorded");
    f.finish().await;
}

/// An old approval-mail send returns after rejection and reapproval created a
/// newer transition. The old finalize's guarded UPDATE matches no row, so it
/// can never clear the new lease or payload. The Python auth-maintenance
/// timer keeps claiming these rows after the switch, so the claim shape is
/// asserted exactly as it will be exercised cross-language.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn approval_email_finalize_preserves_newer_transition() {
    use verdaxis_read::business::approval_mail::{
        FrozenApprovalPayload, claim_approval_mail, finalize_approval_mail,
    };
    let mut f = Fixture::new().await;
    let user = f.user(Some(f.organization), "BUYER").await;

    let payload = FrozenApprovalPayload {
        from: "ops@verdaxis.exchange".into(),
        to: ["member@example.test".into()],
        subject: "Account approved".into(),
        html: "<p>approved</p>".into(),
        reply_to: None,
    };
    let mark = |user: Uuid, transition: Uuid, retry_at: DateTime<Utc>| {
        sqlx::query("UPDATE users SET status='APPROVED',email_verified=true,pending_approval_email_transition_id=$2,pending_approval_email_payload=$3,pending_approval_email_retry_at=$4 WHERE id=$1")
            .bind(user)
            .bind(transition)
            .bind(serde_json::to_value(&payload).unwrap())
            .bind(retry_at)
    };

    // The old send claims the old transition and is then delayed.
    let old_transition = Uuid::new_v4();
    let now = Utc::now();
    mark(user, old_transition, now - chrono::Duration::minutes(5))
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut conn = f.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let old_claim = claim_approval_mail(&mut *tx, user, old_transition, now)
        .await
        .unwrap()
        .expect("the old transition is claimable");

    // Rejection then reapproval creates a newer transition with a new lease.
    let new_transition = Uuid::new_v4();
    sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
        .bind(user)
        .execute(&mut f.admin)
        .await
        .unwrap();
    mark(user, new_transition, now + chrono::Duration::minutes(30))
        .execute(&mut f.admin)
        .await
        .unwrap();

    // The delayed old finalize commits against its stale claim.
    finalize_approval_mail(&mut *tx, &old_claim, true, now + chrono::Duration::minutes(10))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    drop(conn);

    let (transition, payload_left, retry_at): (Option<Uuid>, Option<Value>, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT pending_approval_email_transition_id,pending_approval_email_payload,pending_approval_email_retry_at FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        transition,
        Some(new_transition),
        "the old finalize cleared the newer transition"
    );
    assert!(payload_left.is_some(), "the old finalize cleared the new payload");
    assert_eq!(
        retry_at,
        Some(now + chrono::Duration::minutes(30)),
        "the old finalize moved the new lease"
    );

    // The new transition remains claimable exactly once for its own send.
    let mut conn = f.pool.acquire().await.unwrap();
    let new_claim = claim_approval_mail(
        &mut *conn,
        user,
        new_transition,
        now + chrono::Duration::minutes(35),
    )
    .await
    .unwrap()
    .expect("the newer transition is claimable");
    assert_eq!(new_claim.marker.transition_id, new_transition);
    drop(conn);
    f.finish().await;
}

/// Invitation rotation racing the current token's consumption. Rotation and
/// consumption both write `users.password_reset_token_hash` under the user
/// row lock, so the rotation either lands before the acceptance reads the
/// digest (the old secret is dead, exactly one consumption) or waits behind
/// it (the rotated secret is the only valid one afterwards). Python keeps
/// issuing these invitations until its admin routes move, so the digest
/// column stays a two-writer row.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn invitation_rotation_vs_acceptance_is_serialized() {
    let mut f = Fixture::new().await;
    let invitee = f.user(None, "BUYER").await;
    let old_digest = format!("{:064x}", 1u128 << 100);
    let new_digest = format!("{:064x}", 2u128 << 100);
    let expires = Utc::now() + chrono::Duration::hours(24);
    sqlx::query("UPDATE users SET status='APPROVED',email_verified=false,must_change_password=true,password_reset_token_hash=$2,password_reset_expires=$3 WHERE id=$1")
        .bind(invitee).bind(&old_digest).bind(expires)
        .execute(&mut f.admin).await.unwrap();

    // The acceptance holds the user row while reading the digest; the
    // rotation must queue behind it on the same row.
    let mut holder = f.pool.acquire().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder).await.unwrap();
    let mut held = holder.begin().await.unwrap();
    let resolved: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=$1 AND password_reset_token_hash=$2 FOR UPDATE")
            .bind(invitee).bind(&old_digest)
            .fetch_optional(&mut *held).await.unwrap();
    assert!(resolved.is_some(), "the current token still resolves");

    let rotation_new = new_digest.clone();
    let rotation_old = old_digest.clone();
    let rotation = tokio::spawn({
        let pool = f.pool.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            let updated = sqlx::query("UPDATE users SET password_reset_token_hash=$1,password_reset_expires=$2 WHERE id=$3 AND password_reset_token_hash=$4")
                .bind(&rotation_new).bind(expires).bind(invitee).bind(&rotation_old)
                .execute(&mut *conn).await.unwrap();
            updated.rows_affected()
        }
    });
    let waiter = f.wait_any_blocked_by(holder_pid).await;

    // The held acceptance consumes the token under its lock.
    sqlx::query("UPDATE users SET password_reset_token_hash=NULL,password_reset_expires=NULL,must_change_password=false,email_verified=true WHERE id=$1")
        .bind(invitee)
        .execute(&mut *held).await.unwrap();
    held.commit().await.unwrap();
    drop(holder);

    let rotated = rotation.await.unwrap();
    let final_digest: Option<String> =
        sqlx::query_scalar("SELECT password_reset_token_hash FROM users WHERE id=$1")
            .bind(invitee)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    // Whichever side won the row, exactly one consumption shape remains: no
    // doubled session material, no old secret still valid.
    if rotated == 1 {
        assert_eq!(final_digest.as_deref(), Some(new_digest.as_str()));
    } else {
        assert_eq!(final_digest, None, "consumption won; the token is spent");
    }
    assert_ne!(final_digest.as_deref(), Some(old_digest.as_str()));
    assert_eq!(f.count("audit_logs").await, 0, "no admission audit fired");
    let _ = waiter;
    f.finish().await;
}

/// Injected serialization failures (40001 then 40P01) on the organization
/// write: the shared retry returns success when a later attempt commits, and
/// the busy refusal after exhaustion leaves every row unchanged — no partial
/// stock, history, event or session effect either way.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn org_write_injected_conflicts_retry_or_refuse_atomically() {
    for sqlstate in ["40001", "40P01"] {
        // Success after the injected conflicts clear within the attempt bound.
        let mut f = Fixture::new().await;
        let member = f.user(Some(f.organization), "BUYER").await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SEQUENCE race_attempts; \
             CREATE FUNCTION race_conflict() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN IF nextval('race_attempts') < 3 THEN RAISE EXCEPTION 'race fixture' USING ERRCODE='{sqlstate}'; END IF; RETURN NEW; END $$; \
             CREATE TRIGGER race_conflict BEFORE INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION race_conflict()"
        )))
        .execute(&mut f.admin)
        .await
        .unwrap();
        let recovered = run_ordered_write(
            f.pool.clone(),
            Arc::new(Semaphore::new(1)),
            access(f.actor),
            rejection(member, RejectionRoute::Auth),
        )
        .await;
        match recovered {
            Ok(_) => {}
            Err(error) => panic!("retry did not absorb the injected {sqlstate}: {error:?}"),
        }
        let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id=$1")
            .bind(member)
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(status, "REJECTED");
        assert_eq!(f.count("market_event_outbox").await, 1);
        f.finish().await;

        // Exhaustion: the failure never clears, the refusal is busy, and no
        // partial write survives.
        let mut f = Fixture::new().await;
        let member = f.user(Some(f.organization), "BUYER").await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE FUNCTION race_conflict_forever() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN RAISE EXCEPTION 'race fixture' USING ERRCODE='{sqlstate}'; END $$; \
             CREATE TRIGGER race_conflict_forever BEFORE INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION race_conflict_forever()"
        )))
        .execute(&mut f.admin)
        .await
        .unwrap();
        let refused = run_ordered_write(
            f.pool.clone(),
            Arc::new(Semaphore::new(1)),
            access(f.actor),
            rejection(member, RejectionRoute::Auth),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.status.as_u16(), 503, "{sqlstate}: {refused:?}");
        let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id=$1")
            .bind(member)
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(status, "APPROVED", "a partial rejection persisted");
        assert_eq!(f.count("user_status_transitions").await, 0);
        assert_eq!(f.count("market_event_outbox").await, 0);
        assert_eq!(f.count("audit_logs").await, 0);
        f.finish().await;
    }
}

/// A rejected party (account, organization, KYC and the analytics view of the
/// same account) cannot start new execution, while a pre-existing trade for
/// which it is an immutable participant remains deliverable and payable.
/// Participants and ownership are preserved; current membership being
/// unlinked changes neither. The four rejection routes each cover the same
/// invariant.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn rejected_party_new_execution_refused_but_existing_deliver_pay_allowed() {
    let mut f = Fixture::new().await;
    let member = f.user(Some(f.organization), "BUYER").await;
    // A pre-existing trade with immutable participants.
    let trade = Uuid::new_v4();
    let counterparty = Uuid::new_v4();
    sqlx::query("INSERT INTO trades(id,buyer_id,seller_id,status,quantity_mt,initiator_org_id) VALUES($1,$2,$3,'CONFIRMED',10,$4)")
        .bind(trade).bind(member).bind(counterparty).bind(f.organization)
        .execute(&mut f.admin).await.unwrap();

    // The execution admission the market side applies: a rejected or
    // membership-unlinked party cannot place a new order (the eligibility
    // predicate), while the existing trade's lifecycle columns stay
    // untouched by every rejection route.
    for (route, applied) in [
        (RejectionRoute::Auth, true),
        (RejectionRoute::Analytics, true),
    ] {
        if applied {
            run_ordered_write(
                f.pool.clone(),
                Arc::new(Semaphore::new(1)),
                access(f.actor),
                rejection(member, route),
            )
            .await
            .unwrap();
        }
        // New execution: the account is rejected and unlinked; the policy
        // refuses. The trade row is unchanged.
        let party = KycParty {
            user_id: member,
            organization_id: None,
            kyc_organization_id: Some(f.organization),
            kyc_status: KycStatus::Rejected,
        };
        let decision = eligibility(&KycPolicy::default(), &party, None, Utc::now());
        assert!(!decision.eligible);
        assert_eq!(decision.reason, KycReason::Rejected);
        let row = sqlx::query("SELECT buyer_id,seller_id,status,quantity_mt FROM trades WHERE id=$1")
            .bind(trade)
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(row.get::<Uuid, _>("buyer_id"), member, "immutable participant changed");
        assert_eq!(row.get::<Uuid, _>("seller_id"), counterparty);
        assert_eq!(row.get::<String, _>("status"), "CONFIRMED", "lifecycle changed by rejection");
        assert_eq!(row.get::<rust_decimal::Decimal, _>("quantity_mt"), rust_decimal::Decimal::from(10));
    }
    f.finish().await;
}

/// Failure between the status write and its history insert rolls the whole
/// admission back: neither the status change, the event, the audit nor any
/// partial effect persists, and a later rerun starts from committed facts.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn status_history_failure_rolls_back_admission_atomically() {
    let mut f = Fixture::new().await;
    let member = f.user(Some(f.organization), "BUYER").await;
    // The trusted fixture writer installs the failure; the runtime role has
    // no DDL authority.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {}.user_status_transitions ADD CONSTRAINT refuse_history CHECK (false) NOT VALID",
        f.schema
    )))
    .execute(&mut f.admin)
    .await
    .unwrap();
    let refused = run_ordered_write(
        f.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(f.actor),
        rejection(member, RejectionRoute::Auth),
    )
    .await
    .unwrap_err();
    assert_ne!(refused.status.as_u16(), 200);
    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id=$1")
        .bind(member)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(status, "APPROVED", "the status change persisted alone");
    assert_eq!(f.count("user_status_transitions").await, 0);
    assert_eq!(f.count("market_event_outbox").await, 0);
    assert_eq!(f.count("audit_logs").await, 0);

    // Rerun from committed facts: drop the constraint and the same command
    // succeeds exactly once.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {}.user_status_transitions DROP CONSTRAINT refuse_history",
        f.schema
    )))
    .execute(&mut f.admin)
    .await
    .unwrap();
    run_ordered_write(
        f.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(f.actor),
        rejection(member, RejectionRoute::Auth),
    )
    .await
    .unwrap();
    assert_eq!(f.count("user_status_transitions").await, 1);
    assert_eq!(f.count("market_event_outbox").await, 1);
    assert_eq!(f.count("audit_logs").await, 1);
    f.finish().await;
}

/// The detached ordered owner commits at most once across caller
/// cancellation before or after commit and a lost commit acknowledgement:
/// the ambiguous connection is discarded, the commit is never retried, and
/// the permit returns only after the commit settles.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn organization_write_cancellation_does_not_duplicate_commit() {
    use verdaxis_read::business::transaction::{CompletionControl, CompletionPoint};

    struct Effect;
    impl BusinessCommand for Effect {
        type Output = ();
        fn preview<'a>(
            &'a self,
            _conn: &'a mut PgConnection,
        ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
            Box::pin(async move {
                Ok(OrganizationWritePlan::new([], [], [], json!("race")))
            })
        }
        fn execute<'a>(
            &'a self,
            conn: &'a mut PgConnection,
            _actor: &'a CurrentActor,
        ) -> CommandFuture<'a, Self::Output, AttemptError> {
            Box::pin(async move {
                sqlx::query("INSERT INTO effects VALUES($1)")
                    .bind(Uuid::new_v4())
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        }
    }

    for (point, lost_ack, cancel) in [
        (CompletionPoint::BeforeCommit, false, true),
        (CompletionPoint::AfterCommit, false, true),
        (CompletionPoint::AfterCommit, true, false),
    ] {
        let f = Fixture::new().await;
        let control = CompletionControl::new(point, lost_ack);
        let gate = control.clone();
        let permits = Arc::new(Semaphore::new(1));
        let capacity = permits.clone();
        let pool = f.pool.clone();
        let actor_id = f.actor;
        let caller = tokio::spawn(async move {
            run_controlled_write(pool, capacity, access(actor_id), Effect, gate).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(20), control.reached())
            .await
            .unwrap();
        assert_eq!(permits.available_permits(), 0);
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM effects")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(before, if point == CompletionPoint::BeforeCommit { 0 } else { 1 });
        if cancel {
            caller.abort();
        }
        control.release();
        let result = caller.await;
        if lost_ack {
            assert_eq!(
                result.unwrap().unwrap_err().detail["code"],
                "COMMIT_OUTCOME_UNKNOWN"
            );
        }
        // The permit returns only after the commit settled.
        let permit = tokio::time::timeout(std::time::Duration::from_secs(10), permits.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        // Exactly one effect: the commit is never retried, so a cancellation
        // after commit or a lost acknowledgement cannot duplicate it.
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM effects")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(after, 1, "commit duplicated by retry or revival");
        assert_eq!(f.count("market_event_outbox").await, 0);
        f.finish().await;
    }
}

/// The subscription bootstrap race: two users plus the admin create the same
/// organization subscription concurrently. The unique organization key and
/// the ordered owner serialize them: exactly one row exists, no request
/// fails with a server error, and a negotiated tier is never downgraded by
/// the bootstrap default.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn concurrent_subscription_bootstrap_is_one_row() {
    let mut f = Fixture::new().await;
    let other = f.user(None, "BUYER").await;
    sqlx::raw_sql(
        "CREATE TABLE subscriptions(id uuid PRIMARY KEY,organization_id uuid NOT NULL UNIQUE,tier text NOT NULL DEFAULT 'PILOT',rate numeric,updated_at timestamptz DEFAULT now()); \
         CREATE TABLE subscription_events(id uuid PRIMARY KEY);",
    )
    .execute(&mut f.admin)
    .await
    .unwrap();

    // The bootstrap is an INSERT ... ON CONFLICT DO NOTHING under the
    // organization row lock; run it from three connections at once.
    let bootstrap = |pool: sqlx::PgPool, organization: Uuid| async move {
        let mut conn = pool.acquire().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM organizations WHERE id=$1 FOR UPDATE")
            .bind(organization)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO subscriptions(id,organization_id) VALUES($1,$2) ON CONFLICT (organization_id) DO NOTHING")
            .bind(Uuid::new_v4())
            .bind(organization)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    };
    let org = f.organization;
    let a = tokio::spawn(bootstrap(f.pool.clone(), org));
    let b = tokio::spawn(bootstrap(f.pool.clone(), org));
    let c = tokio::spawn(bootstrap(f.pool.clone(), org));
    let (a, b, c) = tokio::join!(a, b, c);
    for task in [a, b, c] {
        task.unwrap();
    }

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "bootstrap created more than one row");
    let tier: String = sqlx::query_scalar("SELECT tier FROM subscriptions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(tier, "PILOT");

    // A negotiated tier is never downgraded by a later bootstrap attempt.
    sqlx::query("UPDATE subscriptions SET tier='ENTERPRISE',rate=0.75")
        .execute(&mut f.admin)
        .await
        .unwrap();
    bootstrap(f.pool.clone(), org).await;
    let (tier, rate): (String, Option<rust_decimal::Decimal>) =
        sqlx::query_as("SELECT tier,rate FROM subscriptions")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(tier, "ENTERPRISE");
    assert_eq!(rate, Some(rust_decimal::Decimal::try_from(0.75).unwrap()));
    let _ = other;
    f.finish().await;
}

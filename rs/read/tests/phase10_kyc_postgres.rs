//! Phase 10 Task 15: KYC grace notices.
//!
//! The offline cases cover the refusals that must happen before any database
//! contact and the mail contract. The PostgreSQL cases (feature
//! `contract-tests`) build a private schema per test on a disposable database
//! and send only to a loopback provider stub; no real mail is ever sent.
use chrono::{TimeZone, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;
use verdaxis_read::cli::jobs::kyc::run_with;
use verdaxis_read::cli::mail::kyc::{ClaimedNotice, idempotency_key, render_payload};
use verdaxis_read::cli::{Environment, Exit, IdentityArgs, JobArgs, JobKind};
use verdaxis_read::crud::referrals::invite::MailClient;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn args(environment: Environment, sha: &str, apply: bool) -> JobArgs {
    JobArgs {
        kind: JobKind::KycNotices,
        identity: IdentityArgs {
            environment,
            release_sha: sha.into(),
        },
        apply,
    }
}

/// The job environment of a staging release whose notices may run.
fn vars() -> HashMap<String, String> {
    [
        ("ENVIRONMENT", "staging"),
        ("RELEASE_SHA", SHA),
        (
            "DATABASE_URL",
            "postgres://verdaxis_app_staging:unused@127.0.0.1:1/verdaxis_rust_test",
        ),
        ("FRONTEND_URL", "https://app.example.test/"),
        ("EMAIL_FROM", "Verdaxis <noreply@example.test>"),
        ("RESEND_API_KEY", "test-only-key"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

/// One private scratch directory, removed with its contents.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("verdaxis-kyc-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&path).expect("create a scratch directory");
        Self(path)
    }

    fn state_path(&self) -> PathBuf {
        self.0.join("staging.state")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the job with an opener that records whether a pool was requested. The
/// opener always refuses, so a run that reaches it ends in `Precondition`.
async fn run_recording_pool_use(
    job: &JobArgs,
    vars: &HashMap<String, String>,
    deploy_state: &std::path::Path,
    mail: Option<MailClient>,
) -> (Exit, bool) {
    let opened = Arc::new(AtomicBool::new(false));
    let flag = opened.clone();
    let exit = run_with(job, vars, deploy_state, mail, move |_, _| {
        flag.store(true, Ordering::SeqCst);
        std::future::ready(Err(Exit::Precondition))
    })
    .await;
    (exit, opened.load(Ordering::SeqCst))
}

#[tokio::test]
async fn kyc_gate_disabled_exits_before_pool() {
    let scratch = TempDir::new();
    let job = args(Environment::Staging, SHA, true);
    for token in ["false", "0", "no", "off", "FALSE", " False "] {
        let mut disabled = vars();
        disabled.insert("KYC_EXECUTION_GATE_ENABLED".into(), token.into());
        let (exit, opened) = run_recording_pool_use(
            &job,
            &disabled,
            &scratch.state_path(),
            Some(MailClient::disabled()),
        )
        .await;
        assert_eq!(exit, Exit::Usage, "gate token {token:?}");
        assert!(
            !opened,
            "no pool is opened while the gate is off ({token:?})"
        );
    }
    // Controls: the same run with the gate absent (default on) or on reaches the
    // pool opener, so the refusals above are the gate and nothing else.
    for gate in [None, Some("true"), Some("1")] {
        let mut enabled = vars();
        if let Some(token) = gate {
            enabled.insert("KYC_EXECUTION_GATE_ENABLED".into(), token.into());
        }
        let (exit, opened) = run_recording_pool_use(
            &job,
            &enabled,
            &scratch.state_path(),
            Some(MailClient::disabled()),
        )
        .await;
        assert_eq!(exit, Exit::Precondition);
        assert!(opened, "gate {gate:?} reaches the pool opener");
    }
    // A malformed switch is not read as on or off.
    let mut malformed = vars();
    malformed.insert("KYC_EXECUTION_GATE_ENABLED".into(), "maybe".into());
    let (exit, opened) = run_recording_pool_use(
        &job,
        &malformed,
        &scratch.state_path(),
        Some(MailClient::disabled()),
    )
    .await;
    assert_eq!(exit, Exit::Precondition);
    assert!(!opened);
}

#[tokio::test]
async fn kyc_identity_is_checked_before_the_gate_and_the_pool() {
    let scratch = TempDir::new();
    let mut gate_off = vars();
    gate_off.insert("KYC_EXECUTION_GATE_ENABLED".into(), "false".into());
    let mut missing_environment = vars();
    missing_environment.remove("ENVIRONMENT");
    let mut missing_sha = vars();
    missing_sha.remove("RELEASE_SHA");
    let mut short_sha = vars();
    short_sha.insert("RELEASE_SHA".into(), "0123456".into());
    let cases = [
        (
            "other environment",
            args(Environment::Production, SHA, true),
            vars(),
        ),
        (
            "other release",
            args(Environment::Staging, OTHER_SHA, true),
            vars(),
        ),
        (
            "unset environment",
            args(Environment::Staging, SHA, true),
            missing_environment,
        ),
        (
            "unset release",
            args(Environment::Staging, SHA, true),
            missing_sha,
        ),
        (
            "abbreviated release",
            args(Environment::Staging, "0123456", true),
            short_sha,
        ),
        // The identity refusal wins over the disabled gate (exit 3, not 2).
        (
            "wrong release with the gate off",
            args(Environment::Staging, OTHER_SHA, true),
            gate_off,
        ),
    ];
    for (label, job, vars) in cases {
        let (exit, opened) = run_recording_pool_use(
            &job,
            &vars,
            &scratch.state_path(),
            Some(MailClient::disabled()),
        )
        .await;
        assert_eq!(exit, Exit::Precondition, "{label}");
        assert!(!opened, "{label}: no pool before the identity matches");
    }
}

#[tokio::test]
async fn kyc_job_requires_apply_before_the_pool() {
    let scratch = TempDir::new();
    let (exit, opened) = run_recording_pool_use(
        &args(Environment::Staging, SHA, false),
        &vars(),
        &scratch.state_path(),
        Some(MailClient::disabled()),
    )
    .await;
    assert_eq!(exit, Exit::Usage);
    assert!(!opened);
}

#[tokio::test]
async fn kyc_job_refuses_while_deploy_state_exists() {
    let scratch = TempDir::new();
    let job = args(Environment::Staging, SHA, true);
    let state = scratch.state_path();

    std::fs::write(&state, b"blocked").unwrap();
    let (exit, opened) =
        run_recording_pool_use(&job, &vars(), &state, Some(MailClient::disabled())).await;
    assert_eq!((exit, opened), (Exit::Precondition, false), "state file");
    std::fs::remove_file(&state).unwrap();

    // A dangling link is still present state.
    std::os::unix::fs::symlink(scratch.0.join("missing-target"), &state).unwrap();
    let (exit, opened) =
        run_recording_pool_use(&job, &vars(), &state, Some(MailClient::disabled())).await;
    assert_eq!((exit, opened), (Exit::Precondition, false), "dangling link");
    std::fs::remove_file(&state).unwrap();

    // Control: with the state absent the run reaches the pool opener.
    let (exit, opened) =
        run_recording_pool_use(&job, &vars(), &state, Some(MailClient::disabled())).await;
    assert_eq!((exit, opened), (Exit::Precondition, true));
}

#[tokio::test]
async fn kyc_live_send_requires_separate_founder_authorization() {
    let scratch = TempDir::new();
    let job = args(Environment::Staging, SHA, true);
    // The gate is on, `--apply` is given and a provider key is present: that
    // alone must not select the live provider.
    let ready = vars();
    assert!(!ready.contains_key("KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED"));
    let (exit, opened) = run_recording_pool_use(&job, &ready, &scratch.state_path(), None).await;
    assert_eq!((exit, opened), (Exit::Usage, false), "no authorization");

    for token in ["false", "0", ""] {
        let mut declined = vars();
        declined.insert("KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED".into(), token.into());
        let (exit, opened) =
            run_recording_pool_use(&job, &declined, &scratch.state_path(), None).await;
        assert_eq!((exit, opened), (Exit::Usage, false), "token {token:?}");
    }

    // Authorization without a provider key still refuses before the pool.
    let mut keyless = vars();
    keyless.insert(
        "KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED".into(),
        "true".into(),
    );
    keyless.remove("RESEND_API_KEY");
    let (exit, opened) = run_recording_pool_use(&job, &keyless, &scratch.state_path(), None).await;
    assert_eq!(
        (exit, opened),
        (Exit::Precondition, false),
        "no provider key"
    );
    keyless.insert("RESEND_API_KEY".into(), String::new());
    let (exit, opened) = run_recording_pool_use(&job, &keyless, &scratch.state_path(), None).await;
    assert_eq!(
        (exit, opened),
        (Exit::Precondition, false),
        "empty provider key"
    );

    // Control: authorization plus a key reaches the pool opener. The opener
    // refuses here, so no request leaves the process.
    let mut authorized = vars();
    authorized.insert(
        "KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED".into(),
        "true".into(),
    );
    let (exit, opened) =
        run_recording_pool_use(&job, &authorized, &scratch.state_path(), None).await;
    assert_eq!((exit, opened), (Exit::Precondition, true), "authorized");
}

#[test]
fn kyc_payload_and_idempotency_key_follow_the_mail_contract() {
    let grace = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
    assert_eq!(
        idempotency_key(grace),
        "kyc-grace-11111111-2222-3333-4444-555555555555"
    );
    let expires = Utc.with_ymd_and_hms(2026, 11, 14, 9, 5, 0).unwrap();
    let payload = render_payload(
        "Verdaxis <noreply@example.test>",
        "https://app.example.test/",
        "grace@example.test",
        expires,
    );
    assert_eq!(payload["from"], "Verdaxis <noreply@example.test>");
    assert_eq!(payload["reply_to"], "admin@verdaxis.exchange");
    assert_eq!(payload["to"], serde_json::json!(["grace@example.test"]));
    assert_eq!(
        payload["subject"],
        "Complete your KYC to keep trading on Verdaxis"
    );
    let html = payload["html"].as_str().unwrap();
    assert!(
        html.contains("Complete KYC by 14 November 2026 09:05 UTC to keep trading on Verdaxis.")
    );
    assert!(html.contains("href=\"https://app.example.test/kyc\""));
    assert!(
        !html.contains("//kyc"),
        "the trailing slash is trimmed once"
    );
    assert!(!html.contains("{{"), "every template token is replaced");
    assert!(
        !html.contains("grace@example.test"),
        "the address is not rendered into the body"
    );

    // Frontend text is escaped, never trusted as markup.
    let hostile = render_payload(
        "Verdaxis <noreply@example.test>",
        "https://app.example.test/?a=1&b=\"2\"<x>",
        "grace@example.test",
        expires,
    );
    let html = hostile["html"].as_str().unwrap();
    assert!(!html.contains("<x>"));
    assert!(html.contains("&amp;b=&quot;2&quot;&lt;x&gt;"));
}

#[test]
fn kyc_claimed_notice_debug_hides_the_address() {
    let claimed = ClaimedNotice {
        grace_id: Uuid::new_v4(),
        email: "grace@example.test".into(),
        expires_at: Utc.with_ymd_and_hms(2026, 11, 14, 9, 5, 0).unwrap(),
    };
    let text = format!("{claimed:?}");
    assert!(!text.contains("grace@example.test"));
    assert!(!text.contains('@'));
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::Value;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::{Connection, PgConnection, PgPool, Row};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU16;
    use std::time::Duration;
    use verdaxis_read::cli::context::RunContext;
    use verdaxis_read::cli::mail::kyc::{
        KycCounts, KycNotice, MAX_PER_INVOCATION, PAGE_SIZE, claim, eligible_page,
    };

    const SENDER: &str = "Verdaxis <noreply@example.test>";
    const FRONTEND: &str = "https://app.example.test";

    // ------------------------------------------------------------------
    // Loopback provider stub: records every request, answers a set status.
    // ------------------------------------------------------------------

    #[derive(Debug, Clone)]
    struct Call {
        idempotency: Option<String>,
        to: String,
    }

    #[derive(Clone)]
    struct Stub {
        calls: Arc<Mutex<Vec<Call>>>,
        status: Arc<AtomicU16>,
        hang: Arc<AtomicBool>,
    }

    async fn handle(
        State(stub): State<Stub>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> StatusCode {
        stub.calls.lock().unwrap().push(Call {
            idempotency: headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            to: body["to"][0].as_str().unwrap_or_default().to_owned(),
        });
        if stub.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        StatusCode::from_u16(stub.status.load(Ordering::SeqCst)).unwrap()
    }

    struct Provider {
        stub: Stub,
        address: std::net::SocketAddr,
        server: tokio::task::JoinHandle<()>,
    }

    impl Provider {
        async fn start() -> Self {
            let stub = Stub {
                calls: Arc::default(),
                status: Arc::new(AtomicU16::new(200)),
                hang: Arc::default(),
            };
            let app = Router::new()
                .route("/emails", post(handle))
                .with_state(stub.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self {
                stub,
                address,
                server,
            }
        }

        fn client(&self) -> MailClient {
            MailClient::loopback(
                format!("http://{}/emails", self.address),
                "test-only-key".into(),
            )
            .unwrap()
        }

        fn answer(&self, status: u16) {
            self.stub.status.store(status, Ordering::SeqCst);
        }

        fn calls(&self) -> Vec<Call> {
            self.stub.calls.lock().unwrap().clone()
        }

        fn recipients(&self) -> Vec<String> {
            let mut recipients: Vec<String> = self.calls().into_iter().map(|c| c.to).collect();
            recipients.sort();
            recipients
        }
    }

    impl Drop for Provider {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    // ------------------------------------------------------------------
    // Disposable schema with the columns the job reads and writes. Grants
    // follow the deployed application policy: the job role reads users and
    // grace rows and may only select, insert and update notice rows.
    // ------------------------------------------------------------------

    struct Fixture {
        admin: PgConnection,
        schema: String,
        database: String,
        staging_url: String,
        runtime_url: String,
        org: Uuid,
    }

    fn url(variable: &str) -> String {
        std::env::var(variable)
            .unwrap_or_else(|_| panic!("{variable} must name a disposable database"))
    }

    impl Fixture {
        async fn new() -> Self {
            let migrator = url("RUST_TEST_MIGRATOR_DATABASE_URL");
            let staging_url = url("RUST_TEST_STAGING_DATABASE_URL");
            let runtime_url = url("RUST_TEST_DATABASE_URL");
            let parsed = ::url::Url::parse(&migrator).unwrap();
            assert_eq!(parsed.host_str(), Some("127.0.0.1"));
            assert!(parsed.path().ends_with("_test"));
            let database = parsed.path().trim_start_matches('/').to_owned();
            let mut admin = PgConnection::connect(&migrator).await.unwrap();
            let schema = format!("p10_kyc_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema};
                 CREATE TABLE organizations(id uuid PRIMARY KEY);
                 CREATE TABLE users(
                    id uuid PRIMARY KEY,
                    email text NOT NULL,
                    role text,
                    status text NOT NULL DEFAULT 'APPROVED',
                    kyc_status text NOT NULL DEFAULT 'PENDING',
                    organization_id uuid REFERENCES organizations(id),
                    kyc_organization_id uuid REFERENCES organizations(id));
                 CREATE TABLE kyc_execution_grace(
                    id uuid PRIMARY KEY,
                    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
                    kyc_status_at_enrollment varchar(20) NOT NULL
                        CHECK (kyc_status_at_enrollment IN ('PENDING','SUBMITTED','APPROVED')),
                    enrolled_at timestamptz NOT NULL,
                    expires_at timestamptz NOT NULL,
                    created_by varchar(64) NOT NULL,
                    CHECK (expires_at = enrolled_at + interval '336 hours'),
                    UNIQUE (user_id, organization_id));
                 CREATE TABLE kyc_grace_notices(
                    grace_id uuid PRIMARY KEY REFERENCES kyc_execution_grace(id) ON DELETE RESTRICT,
                    claimed_at timestamptz NOT NULL,
                    sent_at timestamptz);
                 GRANT USAGE ON SCHEMA {schema} TO rust_app, verdaxis_app, verdaxis_app_staging;
                 GRANT SELECT ON users, kyc_execution_grace
                    TO rust_app, verdaxis_app, verdaxis_app_staging;
                 GRANT SELECT, INSERT, UPDATE ON kyc_grace_notices
                    TO rust_app, verdaxis_app, verdaxis_app_staging;"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            let mut fixture = Self {
                admin,
                schema,
                database,
                staging_url,
                runtime_url,
                org: Uuid::nil(),
            };
            fixture.org = fixture.organization().await;
            fixture
        }

        async fn finish(mut self) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
        }

        async fn pool_for(&self, url: &str, connections: u32) -> PgPool {
            let path = format!("SET search_path TO {}", self.schema);
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
                .connect(url)
                .await
                .unwrap()
        }

        /// A context as the staging application role, the role that deploys.
        async fn context(&self) -> RunContext {
            context_with(
                self.pool_for(&self.staging_url, 2).await,
                Environment::Staging,
            )
        }

        fn notice(&self, provider: &Provider) -> KycNotice {
            KycNotice::new(
                provider.client(),
                SENDER.into(),
                FRONTEND.into(),
                self.database.clone(),
            )
        }

        async fn organization(&mut self) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO organizations(id) VALUES ($1)")
                .bind(id)
                .execute(&mut self.admin)
                .await
                .unwrap();
            id
        }

        async fn user(
            &mut self,
            email: &str,
            organization: Option<Uuid>,
            kyc_status: &str,
            kyc_organization: Option<Uuid>,
        ) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO users(id, email, kyc_status, organization_id, kyc_organization_id)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(email)
            .bind(kyc_status)
            .bind(organization)
            .bind(kyc_organization)
            .execute(&mut self.admin)
            .await
            .unwrap();
            id
        }

        /// A grace row that expires `expires_in` from now (a Postgres interval
        /// such as `'1 hour'` or `'-1 second'`), always 336 hours long.
        async fn grace(&mut self, user: Uuid, organization: Uuid, expires_in: &str) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO kyc_execution_grace
                    (id, user_id, organization_id, kyc_status_at_enrollment,
                     enrolled_at, expires_at, created_by)
                 VALUES ($1, $2, $3, 'PENDING',
                    now() + $4::text::interval - interval '336 hours',
                    now() + $4::text::interval, 'test')",
            )
            .bind(id)
            .bind(user)
            .bind(organization)
            .bind(expires_in)
            .execute(&mut self.admin)
            .await
            .unwrap();
            id
        }

        /// An eligible user (PENDING, bound to the fixture organization) with a
        /// live grace row.
        async fn eligible(&mut self, email: &str) -> Uuid {
            let org = self.org;
            let user = self.user(email, Some(org), "PENDING", None).await;
            self.grace(user, org, "1 hour").await
        }

        /// `n` eligible users in one statement, for the bound cases.
        async fn eligible_many(&mut self, n: i32) {
            sqlx::query(
                "WITH u AS (
                    INSERT INTO users(id, email, kyc_status, organization_id)
                    SELECT gen_random_uuid(), 'bulk' || g || '@example.test', 'PENDING', $1
                    FROM generate_series(1, $2) AS g
                    RETURNING id)
                 INSERT INTO kyc_execution_grace
                    (id, user_id, organization_id, kyc_status_at_enrollment,
                     enrolled_at, expires_at, created_by)
                 SELECT gen_random_uuid(), id, $1, 'PENDING',
                    now() - interval '1 hour',
                    now() - interval '1 hour' + interval '336 hours', 'test'
                 FROM u",
            )
            .bind(self.org)
            .bind(n)
            .execute(&mut self.admin)
            .await
            .unwrap();
        }

        /// Notice rows as `(grace_id, claimed, sent)`.
        async fn notices(&mut self) -> Vec<(Uuid, bool)> {
            sqlx::query("SELECT grace_id, sent_at IS NOT NULL AS sent FROM kyc_grace_notices ORDER BY grace_id")
                .fetch_all(&mut self.admin)
                .await
                .unwrap()
                .into_iter()
                .map(|row| (row.get("grace_id"), row.get("sent")))
                .collect()
        }

        async fn claimed_at(&mut self, grace: Uuid) -> chrono::DateTime<Utc> {
            sqlx::query_scalar("SELECT claimed_at FROM kyc_grace_notices WHERE grace_id = $1")
                .bind(grace)
                .fetch_one(&mut self.admin)
                .await
                .unwrap()
        }
    }

    fn context_with(pool: PgPool, environment: Environment) -> RunContext {
        RunContext {
            environment,
            release_sha: SHA.into(),
            now: Utc::now(),
            pool,
            migrator: None,
        }
    }

    fn counts(sent: u64, failed: u64, skipped: u64) -> KycCounts {
        KycCounts {
            sent,
            failed,
            skipped,
        }
    }

    async fn send(fixture: &Fixture, provider: &Provider) -> KycCounts {
        let context = fixture.context().await;
        fixture
            .notice(provider)
            .send_page(&context, PAGE_SIZE)
            .await
            .expect("a page runs")
    }

    // ------------------------------------------------------------------
    // The permanent claim.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn kyc_claim_is_permanent_and_not_retried() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let grace = fixture.eligible("grace@example.test").await;

        provider.answer(500);
        assert_eq!(send(&fixture, &provider).await, counts(0, 1, 0));
        let calls = provider.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].to, "grace@example.test");
        assert_eq!(
            calls[0].idempotency.as_deref(),
            Some(format!("kyc-grace-{grace}").as_str()),
            "the provider key is the grace id, so a provider-side replay is deduplicated"
        );
        assert_eq!(fixture.notices().await, vec![(grace, false)]);
        let claimed_at = fixture.claimed_at(grace).await;

        // A second run, and a third after the provider recovers, never send.
        assert_eq!(send(&fixture, &provider).await, counts(0, 0, 0));
        provider.answer(200);
        assert_eq!(send(&fixture, &provider).await, counts(0, 0, 0));
        assert_eq!(
            provider.calls().len(),
            1,
            "a claimed notice is never retried"
        );
        assert_eq!(fixture.notices().await, vec![(grace, false)]);
        assert_eq!(fixture.claimed_at(grace).await, claimed_at);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_ambiguous_send_exits_five_and_a_later_run_is_quiet() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let grace = fixture.eligible("grace@example.test").await;
        let job = args(Environment::Staging, SHA, true);
        let scratch = TempDir::new();

        provider.answer(503);
        let pool = fixture.pool_for(&fixture.staging_url, 2).await;
        let exit = run_with(
            &job,
            &with_database(&fixture),
            &scratch.state_path(),
            Some(provider.client()),
            {
                let pool = pool.clone();
                move |environment, _| async move { Ok(context_with(pool, environment)) }
            },
        )
        .await;
        assert_eq!(
            exit,
            Exit::Ambiguous,
            "an unaccepted claimed send is ambiguous"
        );
        assert_eq!(fixture.notices().await, vec![(grace, false)]);

        provider.answer(200);
        let exit = run_with(
            &job,
            &with_database(&fixture),
            &scratch.state_path(),
            Some(provider.client()),
            {
                let pool = pool.clone();
                move |environment, _| async move { Ok(context_with(pool, environment)) }
            },
        )
        .await;
        assert_eq!(
            exit,
            Exit::Ok,
            "nothing is left to send; the claim is not retried"
        );
        assert_eq!(provider.calls().len(), 1);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_job_sends_and_exits_zero() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let first = fixture.eligible("first@example.test").await;
        let second = fixture.eligible("second@example.test").await;
        let job = args(Environment::Staging, SHA, true);
        let scratch = TempDir::new();
        let pool = fixture.pool_for(&fixture.staging_url, 2).await;
        let exit = run_with(
            &job,
            &with_database(&fixture),
            &scratch.state_path(),
            Some(provider.client()),
            move |environment, _| async move { Ok(context_with(pool, environment)) },
        )
        .await;
        assert_eq!(exit, Exit::Ok);
        assert_eq!(
            provider.recipients(),
            vec!["first@example.test", "second@example.test"]
        );
        let mut expected = vec![(first, true), (second, true)];
        expected.sort();
        assert_eq!(fixture.notices().await, expected);
        fixture.finish().await;
    }

    /// The vars of a staging release whose database URL names the fixture's
    /// database, so the target attestation in the job compares real names.
    fn with_database(fixture: &Fixture) -> HashMap<String, String> {
        let mut vars = vars();
        vars.insert("DATABASE_URL".into(), fixture.staging_url.clone());
        vars
    }

    #[tokio::test]
    async fn kyc_sent_at_is_recorded_only_after_an_accepted_send() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        // The provider accepts 200 and 201 only; every other answer leaves the
        // claim without a sent time.
        for (status, accepted) in [
            (200, true),
            (201, true),
            (202, false),
            (400, false),
            (503, false),
        ] {
            provider.answer(status);
            let grace = fixture
                .eligible(&format!("status{status}@example.test"))
                .await;
            let result = send(&fixture, &provider).await;
            assert_eq!(
                result,
                if accepted {
                    counts(1, 0, 0)
                } else {
                    counts(0, 1, 0)
                },
                "status {status}"
            );
            let sent = fixture
                .notices()
                .await
                .into_iter()
                .find(|(id, _)| *id == grace)
                .map(|(_, sent)| sent);
            assert_eq!(sent, Some(accepted), "status {status}");
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_claim_is_committed_before_the_provider_call() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        let user = fixture
            .user("grace@example.test", Some(org), "PENDING", None)
            .await;
        let grace = fixture.grace(user, org, "1 hour").await;
        provider.stub.hang.store(true, Ordering::SeqCst);

        let context = fixture.context().await;
        let notice = fixture.notice(&provider);
        let run = tokio::spawn(async move { notice.send_page(&context, PAGE_SIZE).await });
        tokio::time::timeout(Duration::from_secs(10), async {
            while provider.calls().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the provider call starts");

        // While the provider call is in flight, another connection already sees
        // the committed claim, and the user row is not locked.
        assert_eq!(fixture.notices().await, vec![(grace, false)]);
        sqlx::raw_sql("SET lock_timeout = '2s'; UPDATE users SET kyc_status = 'SUBMITTED'")
            .execute(&mut fixture.admin)
            .await
            .expect("no user row lock is held across the provider call");
        run.abort();
        let _ = run.await;
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_claim_is_permanent_after_crash() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let grace = fixture.eligible("grace@example.test").await;

        // The process dies while the provider call is in flight: the future is
        // dropped with the claim already committed.
        provider.stub.hang.store(true, Ordering::SeqCst);
        let context = fixture.context().await;
        let crashed = tokio::time::timeout(
            Duration::from_millis(700),
            fixture.notice(&provider).send_page(&context, PAGE_SIZE),
        )
        .await;
        assert!(crashed.is_err(), "the run did not finish before the crash");
        assert_eq!(provider.calls().len(), 1);
        assert_eq!(fixture.notices().await, vec![(grace, false)]);

        // The next process finds the claim and sends nothing, even though the
        // provider answers now and the notice was never recorded as sent.
        provider.stub.hang.store(false, Ordering::SeqCst);
        provider.answer(200);
        assert_eq!(send(&fixture, &provider).await, counts(0, 0, 0));
        assert_eq!(provider.calls().len(), 1, "one provider call in total");
        assert_eq!(fixture.notices().await, vec![(grace, false)]);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_claim_from_another_runner_is_respected() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let grace = fixture.eligible("grace@example.test").await;
        // A claim written by an earlier process, for example the Python command.
        sqlx::query("INSERT INTO kyc_grace_notices(grace_id, claimed_at) VALUES ($1, now())")
            .bind(grace)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        assert_eq!(send(&fixture, &provider).await, counts(0, 0, 0));
        assert!(provider.calls().is_empty());
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_concurrent_runs_send_each_notice_once() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        for index in 0..40 {
            fixture.eligible(&format!("race{index}@example.test")).await;
        }
        // Two runners on independent pools, so two real connections.
        let first_context = fixture.context().await;
        let second_context = fixture.context().await;
        let first = fixture.notice(&provider);
        let second = fixture.notice(&provider);
        let (a, b) = tokio::join!(
            first.send_page(&first_context, PAGE_SIZE),
            second.send_page(&second_context, PAGE_SIZE)
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.sent + b.sent, 40, "every notice is sent: {a:?} {b:?}");
        assert_eq!(a.failed + b.failed, 0);
        let mut keys: Vec<_> = provider
            .calls()
            .into_iter()
            .map(|call| call.idempotency.expect("a provider key on every call"))
            .collect();
        assert_eq!(keys.len(), 40, "exactly one provider call per notice");
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 40, "no notice was sent twice");
        assert!(fixture.notices().await.iter().all(|(_, sent)| *sent));
        fixture.finish().await;
    }

    // ------------------------------------------------------------------
    // Who receives a notice.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn kyc_notice_skips_expired_grace() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        for (email, expires_in) in [
            ("expired-a-second-ago@example.test", "-1 second"),
            ("expired-last-month@example.test", "-30 days"),
            ("live@example.test", "1 hour"),
        ] {
            let user = fixture.user(email, Some(org), "PENDING", None).await;
            fixture.grace(user, org, expires_in).await;
        }
        assert_eq!(send(&fixture, &provider).await, counts(1, 0, 0));
        assert_eq!(provider.recipients(), vec!["live@example.test"]);
        assert_eq!(
            fixture.notices().await.len(),
            1,
            "no claim for an expired grace"
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_notice_skips_changed_organization() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        let other = fixture.organization().await;
        // The user moved to another organization after the grace was created.
        let moved = fixture
            .user("moved@example.test", Some(org), "PENDING", None)
            .await;
        fixture.grace(moved, org, "1 hour").await;
        sqlx::query("UPDATE users SET organization_id = $1 WHERE id = $2")
            .bind(other)
            .bind(moved)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        // The user left every organization.
        let unbound = fixture
            .user("unbound@example.test", Some(org), "SUBMITTED", None)
            .await;
        fixture.grace(unbound, org, "1 hour").await;
        sqlx::query("UPDATE users SET organization_id = NULL WHERE id = $1")
            .bind(unbound)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        // Control: still bound to the organization of the grace.
        let kept = fixture
            .user("kept@example.test", Some(org), "PENDING", None)
            .await;
        fixture.grace(kept, org, "1 hour").await;

        assert_eq!(send(&fixture, &provider).await, counts(1, 0, 0));
        assert_eq!(provider.recipients(), vec!["kept@example.test"]);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_notice_sends_to_the_grace_user_not_org_admins() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        // Other members of the same organization, one of them an administrator,
        // have no grace row and receive nothing.
        let admin = fixture
            .user("admin@example.test", Some(org), "APPROVED", Some(org))
            .await;
        sqlx::query("UPDATE users SET role = 'ADMIN' WHERE id = $1")
            .bind(admin)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        fixture
            .user("colleague@example.test", Some(org), "PENDING", None)
            .await;
        let member = fixture
            .user("member@example.test", Some(org), "PENDING", None)
            .await;
        fixture.grace(member, org, "1 hour").await;

        assert_eq!(send(&fixture, &provider).await, counts(1, 0, 0));
        assert_eq!(provider.recipients(), vec!["member@example.test"]);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_notice_skips_rejected_and_keeps_eligible_bound_users() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        for (email, status) in [
            ("pending@example.test", "PENDING"),
            ("submitted@example.test", "SUBMITTED"),
            ("rejected@example.test", "REJECTED"),
        ] {
            let user = fixture.user(email, Some(org), status, None).await;
            fixture.grace(user, org, "1 hour").await;
        }
        assert_eq!(send(&fixture, &provider).await, counts(2, 0, 0));
        assert_eq!(
            provider.recipients(),
            vec!["pending@example.test", "submitted@example.test"]
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_notice_keeps_approved_unbound_and_skips_approved_bound_without_dependency() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        let other = fixture.organization().await;
        // Approved, review not bound to any organization: access depends on
        // grace, so the notice is kept.
        let unbound = fixture
            .user("approved-unbound@example.test", Some(org), "APPROVED", None)
            .await;
        fixture.grace(unbound, org, "1 hour").await;
        // Approved and bound to the current organization: grace no longer
        // matters, so no notice.
        let bound = fixture
            .user(
                "approved-bound@example.test",
                Some(org),
                "APPROVED",
                Some(org),
            )
            .await;
        fixture.grace(bound, org, "1 hour").await;
        // Approved for another organization: the current organization still
        // depends on grace, so the notice is kept.
        let elsewhere = fixture
            .user(
                "approved-elsewhere@example.test",
                Some(org),
                "APPROVED",
                Some(other),
            )
            .await;
        fixture.grace(elsewhere, org, "1 hour").await;

        assert_eq!(send(&fixture, &provider).await, counts(2, 0, 0));
        assert_eq!(
            provider.recipients(),
            vec![
                "approved-elsewhere@example.test",
                "approved-unbound@example.test"
            ]
        );
        let claimed: Vec<Uuid> = fixture
            .notices()
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(claimed.len(), 2, "no claim row for the approved bound user");
        fixture.finish().await;
    }

    // ------------------------------------------------------------------
    // Bounds.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn kyc_notice_pages_are_bounded() {
        assert_eq!((PAGE_SIZE, MAX_PER_INVOCATION), (200, 1000));
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        fixture.eligible_many(1250).await;
        let context = fixture.context().await;
        let notice = fixture.notice(&provider);

        // The page query is ordered and honours its limit.
        let mut conn = context.pool.acquire().await.unwrap();
        let page = eligible_page(&mut conn, 3).await.unwrap();
        assert_eq!(page.len(), 3);
        let mut sorted = page.clone();
        sorted.sort();
        assert_eq!(page, sorted, "ascending by grace id");
        drop(conn);

        assert_eq!(notice.send_page(&context, 7).await.unwrap().sent, 7);
        assert_eq!(
            notice.send_page(&context, 100_000).await.unwrap().sent,
            u64::from(PAGE_SIZE),
            "a larger limit is clamped to one page"
        );
        assert_eq!(provider.calls().len(), 207);

        // One invocation attempts at most 1000 rows even though more are due.
        let job = args(Environment::Staging, SHA, true);
        let scratch = TempDir::new();
        let pool = context.pool.clone();
        let exit = run_with(
            &job,
            &with_database(&fixture),
            &scratch.state_path(),
            Some(provider.client()),
            move |environment, _| async move { Ok(context_with(pool, environment)) },
        )
        .await;
        assert_eq!(exit, Exit::Ok);
        assert_eq!(provider.calls().len(), 207 + 1000);
        let unclaimed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM kyc_execution_grace g
             WHERE NOT EXISTS (SELECT 1 FROM kyc_grace_notices n WHERE n.grace_id = g.id)",
        )
        .fetch_one(&mut fixture.admin)
        .await
        .unwrap();
        assert_eq!(
            unclaimed,
            1250 - 207 - 1000,
            "the rest waits for the next invocation"
        );
        fixture.finish().await;
    }

    // ------------------------------------------------------------------
    // Claim-time recheck and target attestation.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn kyc_claim_rechecks_eligibility_when_it_claims() {
        let mut fixture = Fixture::new().await;
        let org = fixture.org;
        let user = fixture
            .user("late@example.test", Some(org), "PENDING", None)
            .await;
        let grace = fixture.grace(user, org, "1 hour").await;
        let context = fixture.context().await;
        let mut conn = context.pool.acquire().await.unwrap();

        assert_eq!(eligible_page(&mut conn, 10).await.unwrap(), vec![grace]);
        // The user is rejected after the page was read.
        sqlx::query("UPDATE users SET kyc_status = 'REJECTED' WHERE id = $1")
            .bind(user)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        assert!(claim(&mut conn, grace).await.unwrap().is_none());
        assert!(
            fixture.notices().await.is_empty(),
            "no claim for a rejected user"
        );

        // Back to eligible: the claim returns the current address and deadline.
        sqlx::query("UPDATE users SET kyc_status = 'SUBMITTED' WHERE id = $1")
            .bind(user)
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        let claimed = claim(&mut conn, grace)
            .await
            .unwrap()
            .expect("eligible again");
        assert_eq!(claimed.grace_id, grace);
        assert_eq!(claimed.email, "late@example.test");
        assert!(claimed.expires_at > Utc::now());
        // A second claim of the same grace never succeeds.
        assert!(claim(&mut conn, grace).await.unwrap().is_none());
        drop(conn);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn kyc_job_refuses_a_wrong_target_before_the_first_claim() {
        let provider = Provider::start().await;
        let mut fixture = Fixture::new().await;
        fixture.eligible("grace@example.test").await;

        // An undeployed role on the right database.
        let undeployed = context_with(
            fixture.pool_for(&fixture.runtime_url, 2).await,
            Environment::Staging,
        );
        let result = fixture
            .notice(&provider)
            .send_page(&undeployed, PAGE_SIZE)
            .await;
        assert_eq!(result.unwrap_err(), Exit::Precondition);

        // The staging role under the production identity.
        let wrong_environment = context_with(
            fixture.pool_for(&fixture.staging_url, 2).await,
            Environment::Production,
        );
        let result = fixture
            .notice(&provider)
            .send_page(&wrong_environment, PAGE_SIZE)
            .await;
        assert_eq!(result.unwrap_err(), Exit::Precondition);

        // The right role with the wrong expected database name.
        let context = fixture.context().await;
        let wrong_database = KycNotice::new(
            provider.client(),
            SENDER.into(),
            FRONTEND.into(),
            "verdaxis_other_test".into(),
        );
        assert_eq!(
            wrong_database
                .send_page(&context, PAGE_SIZE)
                .await
                .unwrap_err(),
            Exit::Precondition
        );

        assert!(fixture.notices().await.is_empty(), "no claim was written");
        assert!(provider.calls().is_empty(), "no mail was sent");
        // Control: the deployed role on the named database sends.
        assert_eq!(send(&fixture, &provider).await, counts(1, 0, 0));
        fixture.finish().await;
    }
}

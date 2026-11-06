//! Integrated acceptance of individual offboarding across both stacks.
//!
//! Each case clones a migrated template with the converged runtime grants
//! (`rs/read/tests/run_offboarding_acceptance.sh`):
//!
//! - `RUST_OFFBOARDING_ACCEPTANCE_ADMIN_DATABASE_URL`: the fixture owner;
//! - `RUST_OFFBOARDING_ACCEPTANCE_DATABASE_URL`: the runtime role;
//! - `RUST_OFFBOARDING_ACCEPTANCE_TEMPLATE_DATABASE`: the template name;
//! - `RUST_TEMPLATE_ADMIN_DATABASE_URL`: the role that creates the clones.
//!
//! The Rust half is the served router with the release switch on and the
//! stream hub present, over loopback HTTP where a stream is read. The Python
//! half is the unchanged Python application against the same clone: in
//! process through `tests/fixtures/offboarding_acceptance_harness.py`, and as
//! a loopback server for its private streams through
//! `tests/fixtures/offboarding_python_harness.py`, both with the project's
//! interpreter (`RUST_TEST_PYTHON`, default `python3`). Both stacks share one
//! signing key, issuer and audience, as deployed.
//!
//! Lock races use independent connections. The side that must hold its locks
//! first either pauses before its commit (the Rust command) or waits at a
//! fixture gate after its own row locks (a Python request); the test observes
//! the other side's backend waiting for it before it lets the first side go.
//! Run with `--include-ignored --test-threads=1`.
#![cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;

use std::{
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;
use verdaxis_read::{
    AppState, Config,
    auth::{TokenIssuer, VerificationSettings, offboarding::UnusablePasswordHash, verify_access},
    business::{
        admission::AuditContext,
        offboarding::{OffboardAccount, OffboardRequest, UserMarketInvalidation},
        transaction::{CompletionControl, CompletionPoint, run_controlled_write},
    },
    compiled_sha,
    streams::{bus::StreamHub, heartbeat::HeartbeatStatus, http::Streams},
};

const KEY: &str = "synthetic-acceptance-signing-key-at-least-32-characters";
/// Python requires a distinct issuer and audience.
const ISSUER: &str = "verdaxis-acceptance-api";
const AUDIENCE: &str = "verdaxis-acceptance-web";
const PASSWORD: &str = "SyntheticPassword123";
const NEW_PASSWORD: &str = "AnotherSynthetic456";
/// The credentialed browser origin of the Python test environment.
const ORIGIN: &str = "https://test";
const CASE: &str = "CASE-2026/0042";
/// bcrypt's minimum cost keeps fixture password checks short while a row
/// lock is held; verification accepts any cost.
const FIXTURE_HASH_COST: u32 = 4;
/// Both stacks' private streams recheck authorization every 15 seconds.
const STREAM_RECHECK: Duration = Duration::from_secs(15);
/// Allowance for the streams' own loop timeouts and a loaded test host.
const STREAM_SLACK: Duration = Duration::from_secs(3);
/// The application name of the served Rust router's sessions.
const RUST_APPLICATION: &str = "acceptance-rust";
const ACCEPTANCE_HARNESS: &str = "rs/read/tests/fixtures/offboarding_acceptance_harness.py";
const STREAM_HARNESS: &str = "rs/read/tests/fixtures/offboarding_python_harness.py";
const RESULT_PREFIX: &str = "ACCEPTANCE_RESULT ";
const READY_LINE: &str = "ACCEPTANCE_READY";

const ORGANIZATION: Uuid = Uuid::from_u128(0x5000);
const SUBJECT: Uuid = Uuid::from_u128(0x5001);
const COLLEAGUE: Uuid = Uuid::from_u128(0x5002);
const COUNTERPARTY_ORGANIZATION: Uuid = Uuid::from_u128(0x6000);
const COUNTERPARTY: Uuid = Uuid::from_u128(0x6001);
const OTHER_ORGANIZATION: Uuid = Uuid::from_u128(0x6500);
const ADMIN: Uuid = Uuid::from_u128(0x7001);
/// The subject's pending request to join the other organization.
const JOIN_REQUEST: Uuid = Uuid::from_u128(0x7100);
/// A later request to join the subject's own organization.
const REJOIN_REQUEST: Uuid = Uuid::from_u128(0x7101);
const OPEN_ASK: Uuid = Uuid::from_u128(0xa003);
/// The subject's confirmed sale: a settlement obligation that stays.
const CONFIRMED: Uuid = Uuid::from_u128(0xa011);
/// The colleague's confirmed sale.
const COLLEAGUE_CONFIRMED: Uuid = Uuid::from_u128(0xa014);
/// The colleague's open ASK.
const COLLEAGUE_ASK: Uuid = Uuid::from_u128(0xa020);

// ---------------------------------------------------------------------------
// Tokens.
// ---------------------------------------------------------------------------

fn issuer() -> TokenIssuer {
    TokenIssuer::new(KEY, ISSUER, AUDIENCE)
}

/// An access token issued now, as either stack's login issues it.
fn token(user: Uuid) -> String {
    issuer().access(user, Utc::now(), None)
}

/// An access token with chosen issuance claims and a valid expiry.
fn token_with_issuance(user: Uuid, iat_us: Value, iat: Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        &json!({
            "sub": user.to_string(),
            "type": "access",
            "exp": (Utc::now() + TimeDelta::minutes(15)).timestamp(),
            "iat": iat,
            "iat_us": iat_us,
            "iss": ISSUER,
            "aud": AUDIENCE,
        }),
        &EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap()
}

/// The microsecond issuance time of a token either stack issued.
fn issued_at(token: &str) -> DateTime<Utc> {
    let payload = token.split('.').nth(1).expect("JWT payload");
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    DateTime::from_timestamp_micros(claims["iat_us"].as_i64().expect("iat_us")).unwrap()
}

fn offboard_body(expected: Option<Uuid>) -> Value {
    json!({
        "reason_code": "EMPLOYMENT_ENDED",
        "evidence_case_ref": CASE,
        "urgent": false,
        "expected_organization_id": expected,
    })
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// The served Rust application.
// ---------------------------------------------------------------------------

/// The runtime role's pool; its sessions carry [`RUST_APPLICATION`].
async fn runtime_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(6)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::raw_sql("SET TimeZone TO 'UTC'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SELECT set_config('application_name', $1, false)")
                    .bind(RUST_APPLICATION)
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}

/// The served application with the release switch on and the stream hub
/// present, as a deployed process with both enabled.
fn application(runtime_url: &str, pool: PgPool) -> AppState {
    let config: Config = serde_json::from_value(json!({
        "kyc_execution_gate_enabled": false,
        "environment": "test",
        "release_sha": compiled_sha(),
        "migration_revision": ALLOWED_REVISIONS[0],
        "database_url": runtime_url,
        "db_session_time_zone": "UTC",
        "bind": "127.0.0.1:0",
        "read_admission_slots": 6,
        "pool_max": 7,
        "python_budget": 52,
        "configured_max_connections": 100,
        "phase2": {
            "jwt_secret": KEY, "jwt_issuer": ISSUER, "jwt_audience": AUDIENCE,
            "frontend_url": "https://app.example.test", "email_from": "team@example.test",
            "resend_api_key": "synthetic", "auth_domain_datasets": "null",
            "offboarding_enabled": true
        },
    }))
    .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(runtime_url)
        .unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let mut state = AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read: pool, health },
        policy,
    )
    .unwrap();
    state.streams = Some(Arc::new(Streams {
        hub: StreamHub::default(),
        verification: VerificationSettings::new(KEY.into(), None, ISSUER.into(), AUDIENCE.into()),
        heartbeat: HeartbeatStatus::default(),
    }));
    state
}

/// Serve a router on a loopback port, as the deployed process does.
async fn serve(router: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    port
}

async fn send(
    router: &Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json");
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
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

fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap_or_default()
}

fn refused(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

fn deliver_body() -> Value {
    json!({"final_quantity_mt": "2", "final_price_per_mt": "100"})
}

// ---------------------------------------------------------------------------
// The world: one organization with the departing person and a colleague, a
// counterparty organization, another organization and an administrator.
// ---------------------------------------------------------------------------

struct World {
    database: market_database::MarketDatabase,
    admin: PgConnection,
    observer: PgPool,
    pool: PgPool,
    state: AppState,
    served: Router,
}

impl World {
    async fn new() -> Self {
        let database = market_database::MarketDatabase::new("RUST_OFFBOARDING_ACCEPTANCE").await;
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        for fixture in [
            include_str!("fixtures/offboarding_market.sql"),
            include_str!("fixtures/offboarding_composition.sql"),
            include_str!("fixtures/offboarding_acceptance.sql"),
        ] {
            sqlx::raw_sql(fixture).execute(&mut admin).await.unwrap();
        }
        // The Python models load only their current organization types.
        for (organization, kind, member, role) in [
            (Some(ORGANIZATION), "FUEL_SUPPLIER", SUBJECT, "SUPPLIER"),
            (Some(ORGANIZATION), "FUEL_SUPPLIER", COLLEAGUE, "SUPPLIER"),
            (
                Some(COUNTERPARTY_ORGANIZATION),
                "SHIPPING_LINE",
                COUNTERPARTY,
                "BUYER",
            ),
            (None, "", ADMIN, "ADMIN"),
        ] {
            sqlx::query("SELECT offboarding_market_member($1,$2,$3,$4)")
                .bind(organization)
                .bind(kind)
                .bind(member)
                .bind(role)
                .execute(&mut admin)
                .await
                .unwrap();
        }
        sqlx::query("SELECT composition_credentials($1,$2,$3)")
            .bind(SUBJECT)
            .bind(OTHER_ORGANIZATION)
            .bind(JOIN_REQUEST)
            .execute(&mut admin)
            .await
            .unwrap();
        // Addresses both stacks' email validation accepts, and one known
        // password per person.
        sqlx::raw_sql(
            "UPDATE users SET email = 'person-' || right(id::text, 4) || '@northwind-fuels.com'",
        )
        .execute(&mut admin)
        .await
        .unwrap();
        let hash = bcrypt::hash(PASSWORD, FIXTURE_HASH_COST).unwrap();
        for member in [SUBJECT, COLLEAGUE, COUNTERPARTY, ADMIN] {
            sqlx::query("SELECT acceptance_password($1,$2)")
                .bind(member)
                .bind(&hash)
                .execute(&mut admin)
                .await
                .unwrap();
        }
        let observer = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database.admin_url)
            .await
            .unwrap();
        let pool = runtime_pool(&database.runtime_url).await;
        let state = application(&database.runtime_url, pool.clone());
        let served = verdaxis_read::router(state.clone());
        Self {
            database,
            admin,
            observer,
            pool,
            state,
            served,
        }
    }

    /// The subject's open ASK and confirmed sale, and the colleague's
    /// confirmed sale, all to the counterparty.
    async fn market(&mut self) {
        sqlx::query("SELECT offboarding_market_order($1,$2,'ASK',3,3,'OPEN',NULL)")
            .bind(OPEN_ASK)
            .bind(SUBJECT)
            .execute(&mut self.admin)
            .await
            .unwrap();
        for (trade, seller) in [(CONFIRMED, SUBJECT), (COLLEAGUE_CONFIRMED, COLLEAGUE)] {
            sqlx::query("SELECT offboarding_market_trade($1,NULL,NULL,$2,$3,2,'CONFIRMED')")
                .bind(trade)
                .bind(COUNTERPARTY)
                .bind(seller)
                .execute(&mut self.admin)
                .await
                .unwrap();
        }
    }

    async fn email(&mut self, user: Uuid) -> String {
        sqlx::query_scalar("SELECT email FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn account(&mut self, user: Uuid) -> Value {
        sqlx::query_scalar("SELECT acceptance_account($1)")
            .bind(user)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn trade(&mut self, trade: Uuid) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(t) FROM trades t WHERE id=$1")
            .bind(trade)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn call(&mut self, function: &str, arguments: &[Value]) {
        let placeholders: Vec<String> = (1..=arguments.len()).map(|n| format!("${n}")).collect();
        let sql = format!("SELECT {function}({})", placeholders.join(","));
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for argument in arguments {
            query = match argument {
                Value::String(text) => match text.parse::<Uuid>() {
                    Ok(id) => query.bind(id),
                    Err(_) => query.bind(text.clone()),
                },
                other => query.bind(other.clone()),
            };
        }
        query.execute(&mut self.admin).await.unwrap();
    }

    /// The database clock: tokens issued after it are later than any cutoff
    /// taken before.
    async fn wait_past(&mut self, instant: DateTime<Utc>) {
        loop {
            let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
            if now > instant && Utc::now() > instant {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The served offboarding command, as an administrator sends it.
    async fn offboard(&self, subject: Uuid, expected: Option<Uuid>) -> (StatusCode, Value) {
        send(
            &self.served,
            "POST",
            &format!("/api/auth/admin/users/{subject}/offboard"),
            Some(&token(ADMIN)),
            Some(offboard_body(expected)),
        )
        .await
    }

    /// The serving command for one person, as the route builds it, for the
    /// owner's completion seam.
    fn command(
        &self,
        subject: Uuid,
        expected: Option<Uuid>,
        unusable_hash: UnusablePasswordHash,
    ) -> OffboardAccount<
        Arc<UserMarketInvalidation>,
        Arc<impl verdaxis_read::business::admission::AdmissionEvents + use<>>,
    > {
        let service = verdaxis_read::runtime::offboarding::service(&self.state);
        OffboardAccount {
            subject_id: subject,
            request: OffboardRequest::parse(&serde_json::to_vec(&offboard_body(expected)).unwrap())
                .unwrap(),
            unusable_hash,
            audit: AuditContext::default(),
            market: service.market.clone(),
            events: service.events.clone(),
        }
    }

    fn admin_access(&self) -> verdaxis_read::auth::VerifiedAccess {
        verify_access(
            &token(ADMIN),
            &VerificationSettings::new(KEY.into(), None, ISSUER.into(), AUDIENCE.into()),
            Utc::now(),
        )
        .unwrap()
    }

    fn runtime_role(&self) -> String {
        url::Url::parse(&self.database.runtime_url)
            .unwrap()
            .username()
            .to_owned()
    }

    /// Start a Python request in its own interpreter; it waits for
    /// [`PythonCall::go`].
    async fn python(&self, requests: Vec<Value>) -> PythonCall {
        PythonCall::ready(&self.database.runtime_url, requests).await
    }

    /// Run Python requests to completion.
    async fn python_now(&self, requests: Vec<Value>) -> Value {
        self.python(requests).await.go().await.unwrap()
    }
}

// ---------------------------------------------------------------------------
// The unchanged Python application.
// ---------------------------------------------------------------------------

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// SQLAlchemy's asyncpg form of a PostgreSQL URL.
fn python_url(url: &str) -> String {
    let rest = url
        .strip_prefix("postgresql://")
        .or_else(|| url.strip_prefix("postgres://"))
        .expect("a postgres:// or postgresql:// URL");
    format!("postgresql+asyncpg://{rest}")
}

fn python_command(harness: &str, command: &str, database_url: &str) -> Command {
    let root = repository_root();
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut process = Command::new(python);
    process
        .current_dir(&root)
        .env("PYTHONPATH", &root)
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", KEY)
        .env("JWT_ISSUER", ISSUER)
        .env("JWT_AUDIENCE", AUDIENCE)
        .env("KYC_EXECUTION_GATE_ENABLED", "false")
        .env("DATABASE_URL", python_url(database_url))
        .arg(root.join(harness))
        .arg(command);
    process
}

/// One interpreter with its requests loaded, waiting to send them. Its log
/// output goes to a file, shown only when the harness fails, so that it never
/// interleaves with the test runner's own lines.
struct PythonCall {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    log: PathBuf,
}

impl PythonCall {
    async fn ready(database_url: &str, requests: Vec<Value>) -> Self {
        let mut process = python_command(ACCEPTANCE_HARNESS, "requests", database_url);
        let request = json!({ "requests": requests });
        let log =
            std::env::temp_dir().join(format!("offboarding-acceptance-{}.log", Uuid::new_v4()));
        let sink = std::fs::File::create(&log).expect("a harness log file");
        tokio::task::spawn_blocking(move || {
            let mut child = process
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(sink)
                .spawn()
                .expect(
                    "Python interpreter unavailable: set RUST_TEST_PYTHON or put python3 on PATH",
                );
            let mut stdin = child.stdin.take().expect("stdin");
            writeln!(stdin, "{request}").expect("write the requests");
            stdin.flush().unwrap();
            let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
            let mut line = String::new();
            loop {
                line.clear();
                let read = stdout.read_line(&mut line).expect("harness output");
                assert!(
                    read > 0,
                    "the Python harness ended before it was ready: {}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
                if line.trim_end() == READY_LINE {
                    break;
                }
            }
            Self {
                child,
                stdin,
                stdout,
                log,
            }
        })
        .await
        .unwrap()
    }

    /// Send the requests; the result maps each request name to its status,
    /// body and the cookies the response set.
    fn go(self) -> tokio::task::JoinHandle<Value> {
        tokio::task::spawn_blocking(move || {
            let Self {
                mut child,
                mut stdin,
                mut stdout,
                log,
            } = self;
            writeln!(stdin, "go").expect("release the requests");
            drop(stdin);
            let mut output = String::new();
            std::io::Read::read_to_string(&mut stdout, &mut output).expect("harness output");
            let status = child.wait().expect("Python harness finished");
            let messages = std::fs::read_to_string(&log).unwrap_or_default();
            let _ = std::fs::remove_file(&log);
            assert!(
                status.success(),
                "Python harness failed: {status}: {messages}"
            );
            let line = output
                .lines()
                .find_map(|line| line.strip_prefix(RESULT_PREFIX))
                .expect("one result line");
            let result: Value = serde_json::from_str(line).expect("JSON result");
            result["results"].clone()
        })
    }
}

fn python_status(results: &Value, name: &str) -> u16 {
    results[name]["status"]
        .as_u64()
        .unwrap_or_else(|| panic!("no Python result {name}: {results}")) as u16
}

fn python_cookies(results: &Value, name: &str) -> Value {
    results[name]["cookies"].clone()
}

fn python_token(results: &Value, name: &str) -> String {
    results[name]["body"]["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("no access token in {name}: {results}"))
        .to_owned()
}

fn python_login(name: &str, email: &str, password: &str) -> Value {
    json!({"name": name, "method": "POST", "path": "/api/auth/login",
           "form": {"username": email, "password": password}})
}

/// A cookie-authenticated session request (refresh or logout).
fn python_session(name: &str, path: &str, cookies: &Value) -> Value {
    json!({"name": name, "method": "POST", "path": path, "cookies": cookies,
           "headers": {"Origin": ORIGIN}})
}

fn python_me(name: &str, bearer: &str) -> Value {
    json!({"name": name, "method": "GET", "path": "/api/auth/me", "token": bearer})
}

fn python_approve(name: &str, user: Uuid) -> Value {
    json!({"name": name, "method": "PUT", "path": format!("/api/auth/approve/{user}"),
           "token": token(ADMIN)})
}

fn python_join_approval(name: &str, join: Uuid) -> Value {
    json!({"name": name, "method": "PUT",
           "path": format!("/api/auth/organization-joins/{join}/approve"),
           "token": token(ADMIN), "json": {"review_note": "Verified company request"}})
}

fn python_trade(name: &str, trade: Uuid, action: &str, bearer: &str) -> Value {
    let (method, body) = if action == "pay" {
        ("POST", Value::Null)
    } else {
        ("PUT", deliver_body())
    };
    json!({"name": name, "method": method, "path": format!("/api/trades/{trade}/{action}"),
           "token": bearer, "json": body})
}

/// The Python application as a loopback server, for its two private streams.
struct PythonStreams {
    child: Child,
    base: String,
    control: String,
}

impl PythonStreams {
    async fn start(database_url: &str) -> Self {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let control = verdaxis_read::auth::token_bytes::generate_token();
        let child = python_command(STREAM_HARNESS, "serve", database_url)
            .env("OFFBOARDING_HARNESS_PORT", port.to_string())
            .env("OFFBOARDING_HARNESS_TOKEN", &control)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Its log lines would interleave with the test runner's own.
            .stderr(Stdio::null())
            .spawn()
            .expect("Python interpreter unavailable: set RUST_TEST_PYTHON or put python3 on PATH");
        let mut service = Self {
            child,
            base: format!("http://127.0.0.1:{port}"),
            control,
        };
        let started = Instant::now();
        loop {
            if let Some(status) = service.child.try_wait().unwrap() {
                panic!("the Python service exited early: {status}");
            }
            let ready = client()
                .get(format!("{}/__harness/ready", service.base))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            if ready {
                return service;
            }
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "the Python service did not start"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Publish one event on the Python in-process bus: an event in flight.
    async fn publish(&self, channel: &str, event: &str) {
        let response = client()
            .post(format!("{}/__harness/publish", self.base))
            .header("x-harness-token", &self.control)
            .json(&json!({"channel": channel, "event": event, "data": {"marker": event}}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
}

impl Drop for PythonStreams {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

// ---------------------------------------------------------------------------
// Server-sent event streams over loopback HTTP.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Observed {
    frames: Vec<String>,
    closed_at: Option<Instant>,
}

struct EventStream {
    observed: Arc<Mutex<Observed>>,
    reader: tokio::task::JoinHandle<()>,
}

impl EventStream {
    async fn open(url: String) -> Self {
        let response = client().get(&url).send().await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            200,
            "the stream must open: {url}"
        );
        let observed = Arc::new(Mutex::new(Observed::default()));
        let shared = observed.clone();
        let reader = tokio::spawn(async move {
            let mut response = response;
            let mut buffer = String::new();
            loop {
                match tokio::time::timeout(Duration::from_secs(45), response.chunk()).await {
                    Ok(Ok(Some(bytes))) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(end) = buffer.find("\n\n") {
                            let frame = buffer[..end].to_owned();
                            buffer.drain(..end + 2);
                            shared.lock().unwrap().frames.push(frame);
                        }
                    }
                    _ => {
                        shared.lock().unwrap().closed_at = Some(Instant::now());
                        return;
                    }
                }
            }
        });
        Self { observed, reader }
    }

    fn has(&self, needle: &str) -> bool {
        let observed = self.observed.lock().unwrap();
        observed.frames.iter().any(|frame| frame.contains(needle))
    }

    fn closed_at(&self) -> Option<Instant> {
        self.observed.lock().unwrap().closed_at
    }

    fn last_frame(&self) -> String {
        let observed = self.observed.lock().unwrap();
        observed.frames.last().cloned().unwrap_or_default()
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    condition()
}

// ---------------------------------------------------------------------------
// Lock races.
// ---------------------------------------------------------------------------

/// Which side holds the conflicting locks first.
#[derive(Clone, Copy, Debug, PartialEq)]
enum First {
    Python,
    Rust,
}

/// The backend waiting at the fixture gate.
async fn gate_waiter(observer: &PgPool) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let waiting: Option<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
             AND wait_event_type='Lock' AND wait_event='advisory' LIMIT 1",
        )
        .fetch_optional(observer)
        .await
        .unwrap();
        if let Some(pid) = waiting {
            return pid;
        }
        assert!(Instant::now() < deadline, "no request reached the gate");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// A backend waiting for a lock that `holder` holds, with its application.
async fn blocked_by(observer: &PgPool, holder: i32) -> (i32, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let waiting: Option<(i32, String)> = sqlx::query_as(
            "SELECT pid, application_name FROM pg_stat_activity WHERE datname=current_database() \
             AND pid<>$1 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) LIMIT 1",
        )
        .bind(holder)
        .fetch_optional(observer)
        .await
        .unwrap();
        if let Some(waiting) = waiting {
            return waiting;
        }
        assert!(
            Instant::now() < deadline,
            "no backend waited for backend {holder}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// The one runtime transaction idle while open: the command paused before
/// its commit.
async fn paused_transaction(observer: &PgPool, role: &str) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let paused: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
             AND usename=$1 AND state='idle in transaction'",
        )
        .bind(role)
        .fetch_all(observer)
        .await
        .unwrap();
        if let [pid] = paused[..] {
            return pid;
        }
        assert!(Instant::now() < deadline, "{paused:?}");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// The outcome of one race between the offboarding command and Python
/// requests.
struct RaceOutcome {
    python: Value,
    /// The command's answer; `MARKET_BUSY` answers were repeated.
    command: (StatusCode, Value),
}

/// Race the served offboarding of `subject` against Python `requests`.
///
/// Python first: the requests stop at the fixture gate `gate` after their own
/// row locks; the command starts and is seen waiting for the Python backend;
/// then the gate opens. Rust first: the command pauses before its commit with
/// every lock held; the requests start and one is seen waiting for the
/// command's backend; then the command commits.
///
/// A command that waited longer than its 500 ms lock bound answers
/// `MARKET_BUSY` with nothing kept; the administrator then repeats it.
async fn race(
    world: &mut World,
    first: First,
    gate: &str,
    subject: Uuid,
    expected: Option<Uuid>,
    requests: Vec<Value>,
) -> RaceOutcome {
    let case = format!("{gate}, {first:?} first");
    let python = world.python(requests).await;
    let (python, command) = match first {
        First::Python => {
            world.call("acceptance_gate_install", &[json!(gate)]).await;
            let mut keeper = PgConnection::connect(&world.database.admin_url)
                .await
                .unwrap();
            sqlx::query("SELECT acceptance_gate_close()")
                .execute(&mut keeper)
                .await
                .unwrap();
            let python = python.go();
            let holder = gate_waiter(&world.observer).await;
            let served = world.served.clone();
            let command = tokio::spawn(async move {
                send(
                    &served,
                    "POST",
                    &format!("/api/auth/admin/users/{subject}/offboard"),
                    Some(&token(ADMIN)),
                    Some(offboard_body(expected)),
                )
                .await
            });
            let (waiter, application) = blocked_by(&world.observer, holder).await;
            assert_eq!(application, RUST_APPLICATION, "{case}");
            eprintln!("{case}: Rust backend {waiter} waited for Python backend {holder}");
            sqlx::query("SELECT acceptance_gate_open()")
                .execute(&mut keeper)
                .await
                .unwrap();
            keeper.close().await.unwrap();
            let python = python.await.unwrap();
            let mut command = command.await.unwrap();
            world.call("acceptance_gate_remove", &[]).await;
            if command.0 == StatusCode::CONFLICT && code(&command.1) == "MARKET_BUSY" {
                let kept = world.account(subject).await;
                assert!(kept["offboarded_at"].is_null(), "{case}: busy kept nothing");
                assert_eq!(kept["records"], 0, "{case}");
                eprintln!("{case}: the command waited past its lock bound; repeated");
                command = world.offboard(subject, expected).await;
            }
            (python, command)
        }
        First::Rust => {
            let control = CompletionControl::new(CompletionPoint::BeforeCommit, false);
            let paused = tokio::spawn(run_controlled_write(
                world.pool.clone(),
                Arc::new(Semaphore::new(4)),
                world.admin_access(),
                world.command(
                    subject,
                    expected,
                    UnusablePasswordHash::prepare().await.unwrap(),
                ),
                control.clone(),
            ));
            control.reached().await;
            let holder = paused_transaction(&world.observer, &world.runtime_role()).await;
            let python = python.go();
            let (waiter, _) = blocked_by(&world.observer, holder).await;
            eprintln!("{case}: Python backend {waiter} waited for Rust backend {holder}");
            control.release();
            let committed = paused.await.unwrap();
            assert!(
                committed.is_ok(),
                "{case}: {:?}",
                committed.err().map(|error| error.detail)
            );
            (python.await.unwrap(), (StatusCode::OK, Value::Null))
        }
    };
    RaceOutcome { python, command }
}

/// After any offboarding: the account is marked and detached, every session
/// is revoked and no recovery secret remains.
async fn assert_offboarded(world: &mut World, subject: Uuid, case: &str) -> Value {
    let account = world.account(subject).await;
    assert!(!account["offboarded_at"].is_null(), "{case}: {account}");
    assert_eq!(account["status"], "REJECTED", "{case}");
    assert!(account["organization_id"].is_null(), "{case}");
    assert_eq!(account["live_sessions"], 0, "{case}: {account}");
    assert_eq!(account["reset_tokens"], 0, "{case}");
    assert!(
        account["password_reset_token_hash"].is_null(),
        "{case}: {account}"
    );
    assert_eq!(account["records"], 1, "{case}");
    account
}

fn timestamp(value: &Value) -> DateTime<Utc> {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a time: {value}"))
        .parse()
        .unwrap()
}

/// A token issued before the commit is refused by both stacks.
async fn assert_token_refused(world: &World, bearer: &str, case: &str) {
    let (status, body) = send(&world.served, "GET", "/api/auth/me", Some(bearer), None).await;
    assert!(refused(status), "{case}: Rust {status} {body}");
    let results = world.python_now(vec![python_me("me", bearer)]).await;
    assert!(
        matches!(python_status(&results, "me"), 401 | 403),
        "{case}: Python {results}"
    );
}

// ---------------------------------------------------------------------------
// 1. Both stacks: cutoffs, constraints and streams.
// ---------------------------------------------------------------------------

/// One served command ends the person's access in both stacks.
///
/// - Streams: the Python activity and trade streams and the Rust trade stream,
///   both trade streams reconnected with a replay cursor, close with
///   `auth_revoked` within one 15-second recheck of the commit. A reconnect is
///   refused by both. A colleague's stream stays open.
/// - Tokens: an old token, one issued exactly at the cutoff, one from the
///   future and one with malformed issuance claims are refused by Python's
///   normal and owner-cleanup authentication, by Rust bearer validation and by
///   the real Rust cleanup admission (settlement of the person's confirmed
///   trade). A token issued after the cutoff is refused by Rust for the marker.
/// - Constraints: Python approval and join approval, and generic edits by the
///   runtime role, cannot restore status or membership while the marker is set.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn both_stack_cutoffs_constraints_and_streams_hold() {
    let mut world = World::new().await;
    world.market().await;
    let replay: i64 = sqlx::query_scalar(include_str!("fixtures/offboarding_stream_outbox.sql"))
        .bind(Uuid::new_v4())
        .bind(format!("replay-{}", Uuid::new_v4()))
        .bind(ORGANIZATION.to_string())
        .fetch_one(&mut world.admin)
        .await
        .unwrap();
    let cursor = replay - 1;
    let python = PythonStreams::start(&world.database.runtime_url).await;
    let rust = format!("http://127.0.0.1:{}", serve(world.served.clone()).await);
    let stream_token = |user| issuer().stream(user, ORGANIZATION, "test", Utc::now());
    let subject_stream = stream_token(SUBJECT);
    let colleague_stream = stream_token(COLLEAGUE);
    let python_activity =
        |bearer: &str| format!("{}/api/stream/activity?stream_token={bearer}", python.base);
    let python_trades = |bearer: &str| {
        format!(
            "{}/api/stream/trades?stream_token={bearer}&last_event_id={cursor}",
            python.base
        )
    };
    let rust_trades = |bearer: &str| {
        format!("{rust}/api/stream/trades?stream_token={bearer}&last_event_id={cursor}")
    };

    let streams = [
        (
            "Python activity",
            EventStream::open(python_activity(&subject_stream)).await,
        ),
        (
            "Python trades",
            EventStream::open(python_trades(&subject_stream)).await,
        ),
        (
            "Rust trades",
            EventStream::open(rust_trades(&subject_stream)).await,
        ),
    ];
    let colleague_rust = EventStream::open(rust_trades(&colleague_stream)).await;
    // Both trade streams replayed the outbox before the change.
    for (name, stream) in &streams[1..] {
        assert!(
            wait_until(Duration::from_secs(10), || stream.has("synthetic_replay")).await,
            "{name} replayed the outbox"
        );
    }
    assert!(
        wait_until(Duration::from_secs(10), || colleague_rust
            .has("synthetic_replay"))
        .await,
        "the colleague's Rust stream replayed the outbox"
    );
    let mut subscribed = false;
    for _ in 0..10 {
        python
            .publish(&format!("activity:{ORGANIZATION}"), "synthetic_before")
            .await;
        if wait_until(Duration::from_secs(1), || {
            streams[0].1.has("synthetic_before")
        })
        .await
        {
            subscribed = true;
            break;
        }
    }
    assert!(
        subscribed,
        "the Python activity stream receives live events"
    );

    // Before the change both stacks accept the person's current token.
    let old = token(SUBJECT);
    let (status, body) = send(&world.served, "GET", "/api/auth/me", Some(&old), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = world.python_now(vec![python_me("me", &old)]).await;
    assert_eq!(python_status(&results, "me"), 200, "{results}");

    let (status, body) = world.offboard(SUBJECT, Some(ORGANIZATION)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let committed = Instant::now();
    let account = assert_offboarded(&mut world, SUBJECT, "served command").await;
    let cutoff = timestamp(&account["authentication_revoked_at"]);
    assert!(timestamp(&account["password_changed_at"]) >= cutoff);

    // Streams: every private stream of the person closes within one recheck.
    assert!(
        wait_until(STREAM_RECHECK + STREAM_SLACK, || streams
            .iter()
            .all(|(_, stream)| stream.closed_at().is_some()))
        .await,
        "every stream of the person closes within one recheck interval"
    );
    for (name, stream) in &streams {
        let window = stream.closed_at().unwrap().duration_since(committed);
        assert!(
            window <= STREAM_RECHECK + STREAM_SLACK,
            "{name}: {window:?}"
        );
        assert!(
            stream.last_frame().contains("auth_revoked"),
            "{name} ended by the recheck: {}",
            stream.last_frame()
        );
        eprintln!("{name} closed {window:?} after the commit");
    }
    assert!(
        colleague_rust.closed_at().is_none(),
        "a current member keeps the stream"
    );
    for url in [
        python_activity(&subject_stream),
        python_trades(&subject_stream),
        rust_trades(&subject_stream),
    ] {
        let status = client().get(&url).send().await.unwrap().status().as_u16();
        assert!(
            matches!(status, 401 | 403),
            "reconnect refused: {status} {url}"
        );
    }

    // Tokens: old, equal to the cutoff, future and malformed, in both stacks.
    let cutoff_us = cutoff.timestamp_micros();
    let future = Utc::now() + TimeDelta::hours(1);
    let tokens = [
        ("old", old.clone()),
        (
            "equal",
            token_with_issuance(SUBJECT, json!(cutoff_us), json!(cutoff.timestamp())),
        ),
        (
            "future",
            token_with_issuance(
                SUBJECT,
                json!(future.timestamp_micros()),
                json!(future.timestamp()),
            ),
        ),
        (
            "malformed",
            token_with_issuance(SUBJECT, json!("x"), json!("x")),
        ),
    ];
    let checks: Vec<Value> = tokens
        .iter()
        .map(|(name, bearer)| json!({"name": name, "token": bearer}))
        .collect();
    let mut auth = python_command(STREAM_HARNESS, "auth", &world.database.runtime_url);
    let auth = tokio::task::spawn_blocking(move || {
        let mut child = auth
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json!({"tokens": checks}).to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let line = stdout
            .lines()
            .find_map(|line| line.strip_prefix("OFFBOARDING_RESULT "))
            .expect("one result line")
            .to_owned();
        serde_json::from_str::<Value>(&line).unwrap()["tokens"].clone()
    })
    .await
    .unwrap();
    for (name, bearer) in &tokens {
        for path in ["normal", "cleanup"] {
            let outcome = auth[name][path].as_str().unwrap_or_default();
            assert!(
                outcome.starts_with("refused:") || outcome.starts_with("error:"),
                "Python {path} {name}: {outcome}"
            );
        }
        let (status, body) = send(&world.served, "GET", "/api/auth/me", Some(bearer), None).await;
        assert!(refused(status), "Rust bearer {name}: {status} {body}");
        let (status, body) = send(
            &world.served,
            "PUT",
            &format!("/api/trades/{CONFIRMED}/deliver"),
            Some(bearer),
            Some(deliver_body()),
        )
        .await;
        assert!(refused(status), "Rust cleanup {name}: {status} {body}");
    }
    // Issued after the cutoff: no stack issues one, and Rust refuses it for
    // the marker alone.
    world.wait_past(cutoff).await;
    let later = token(SUBJECT);
    assert!(issued_at(&later) > cutoff);
    let (status, body) = send(&world.served, "GET", "/api/auth/me", Some(&later), None).await;
    assert!(
        refused(status),
        "Rust bearer after the cutoff: {status} {body}"
    );
    let (status, body) = send(
        &world.served,
        "PUT",
        &format!("/api/trades/{CONFIRMED}/deliver"),
        Some(&later),
        Some(deliver_body()),
    )
    .await;
    assert!(
        refused(status),
        "Rust cleanup after the cutoff: {status} {body}"
    );
    assert_eq!(world.trade(CONFIRMED).await["status"], "CONFIRMED");

    // Constraints: Python approval and join approval, and generic edits.
    let before = world.account(SUBJECT).await;
    world
        .call(
            "acceptance_join_request",
            &[json!(REJOIN_REQUEST), json!(SUBJECT), json!(ORGANIZATION)],
        )
        .await;
    let results = world
        .python_now(vec![
            python_approve("approve", SUBJECT),
            python_join_approval("join", REJOIN_REQUEST),
        ])
        .await;
    assert!(python_status(&results, "approve") >= 400, "{results}");
    assert!(python_status(&results, "join") >= 400, "{results}");
    let after = world.account(SUBJECT).await;
    assert_eq!(after["status"], before["status"]);
    assert_eq!(after["organization_id"], before["organization_id"]);
    assert_eq!(after["offboarded_at"], before["offboarded_at"]);
    assert_eq!(after["joins"][REJOIN_REQUEST.to_string()], "PENDING");
    eprintln!(
        "Python approval {} and join approval {} refused",
        python_status(&results, "approve"),
        python_status(&results, "join")
    );
    for (edit, constraint) in [
        (
            "UPDATE users SET status='APPROVED' WHERE id=$1",
            "ck_users_offboarded_requires_rejected",
        ),
        (
            "UPDATE users SET organization_id=$2 WHERE id=$1",
            "ck_users_offboarded_requires_detached",
        ),
    ] {
        let error = sqlx::query(edit)
            .bind(SUBJECT)
            .bind(ORGANIZATION)
            .execute(&world.pool)
            .await
            .expect_err("the runtime role cannot restore access");
        let database = error.as_database_error().expect("a database refusal");
        assert_eq!(database.code().as_deref(), Some("23514"), "{edit}");
        assert_eq!(database.constraint(), Some(constraint), "{edit}");
    }
    assert_eq!(world.account(SUBJECT).await, after);
}

// ---------------------------------------------------------------------------
// 2. Return of the same person.
// ---------------------------------------------------------------------------

/// The same work email returns to the existing row, never a second one, and
/// nothing old comes back.
///
/// The Python invitation route refuses the existing address. Clearing the
/// marker is a separate reviewed step that changes nothing else: the account
/// stays rejected and detached, and the old password, access token and
/// refresh session stay refused. Re-approval and a new membership through the
/// existing administrator routes still leave the old session and password
/// refused, and the old organization-bound KYC review stays invalidated. Only
/// a new password from a fresh reset secret signs the person in again, with
/// tokens issued after the old cutoff. The offboarding record stays.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn reinstatement_requires_reviewed_marker_clear_new_invitation_password_and_kyc() {
    let mut world = World::new().await;
    let email = world.email(SUBJECT).await;
    let results = world
        .python_now(vec![python_login("login", &email, PASSWORD)])
        .await;
    assert_eq!(python_status(&results, "login"), 200, "{results}");
    let old_access = python_token(&results, "login");
    let old_cookies = python_cookies(&results, "login");
    assert!(old_cookies["refresh_token"].is_string(), "{results}");

    let (status, body) = world.offboard(SUBJECT, Some(ORGANIZATION)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let offboarded = assert_offboarded(&mut world, SUBJECT, "offboarded").await;
    assert_ne!(offboarded["kyc_status"], "APPROVED");
    assert!(offboarded["kyc_organization_id"].is_null());

    // The same address cannot become a second account.
    let results = world
        .python_now(vec![json!({
            "name": "invite", "method": "POST", "path": "/api/auth/admin/invitations",
            "token": token(ADMIN),
            "json": {"email": email, "first_name": "Returning", "role": "SUPPLIER",
                     "organization_id": ORGANIZATION},
        })])
        .await;
    assert_eq!(python_status(&results, "invite"), 409, "{results}");
    assert_eq!(world.account(SUBJECT).await["rows_with_email"], 1);

    // The reviewed marker clear changes nothing else.
    world
        .call("acceptance_reviewed_marker_clear", &[json!(SUBJECT)])
        .await;
    let cleared = world.account(SUBJECT).await;
    assert!(cleared["offboarded_at"].is_null());
    for field in [
        "status",
        "organization_id",
        "password_hash",
        "password_changed_at",
        "authentication_revoked_at",
        "kyc_status",
        "kyc_organization_id",
        "live_sessions",
        "records",
    ] {
        assert_eq!(cleared[field], offboarded[field], "{field}");
    }
    let stale = |stage: &'static str, world: &World| {
        let (old_access, old_cookies, email) =
            (old_access.clone(), old_cookies.clone(), email.clone());
        let runtime = world.database.runtime_url.clone();
        let served = world.served.clone();
        async move {
            let (status, body) =
                send(&served, "GET", "/api/auth/me", Some(&old_access), None).await;
            assert!(refused(status), "{stage}: Rust old token {status} {body}");
            let results = PythonCall::ready(
                &runtime,
                vec![
                    python_me("me", &old_access),
                    python_session("refresh", "/api/auth/refresh", &old_cookies),
                    python_login("login", &email, PASSWORD),
                ],
            )
            .await
            .go()
            .await
            .unwrap();
            assert!(
                matches!(python_status(&results, "me"), 401 | 403),
                "{stage}: {results}"
            );
            assert_eq!(
                python_status(&results, "refresh"),
                401,
                "{stage}: {results}"
            );
            assert!(
                matches!(python_status(&results, "login"), 401 | 403),
                "{stage}: {results}"
            );
        }
    };
    stale("after the marker clear", &world).await;

    // Re-approval and a new membership restore neither session nor KYC.
    world
        .call(
            "acceptance_join_request",
            &[json!(REJOIN_REQUEST), json!(SUBJECT), json!(ORGANIZATION)],
        )
        .await;
    let results = world
        .python_now(vec![
            python_approve("approve", SUBJECT),
            python_join_approval("join", REJOIN_REQUEST),
        ])
        .await;
    assert_eq!(python_status(&results, "approve"), 200, "{results}");
    assert_eq!(python_status(&results, "join"), 200, "{results}");
    let rejoined = world.account(SUBJECT).await;
    assert_eq!(rejoined["status"], "APPROVED");
    assert_eq!(rejoined["organization_id"], json!(ORGANIZATION));
    assert_ne!(rejoined["kyc_status"], "APPROVED", "new KYC is required");
    assert!(rejoined["kyc_organization_id"].is_null());
    assert_eq!(rejoined["password_hash"], offboarded["password_hash"]);
    stale("after re-approval and rejoin", &world).await;

    // A new password from a fresh reset secret is the only way back in.
    let secret = format!("fresh-reset-{}", Uuid::new_v4());
    world
        .call(
            "acceptance_reset_secret",
            &[json!(SUBJECT), json!(sha256_hex(&secret))],
        )
        .await;
    let results = world
        .python_now(vec![
            json!({"name": "reset", "method": "POST", "path": "/api/auth/reset-password",
                   "json": {"token": secret, "new_password": NEW_PASSWORD}}),
            python_login("login", &email, NEW_PASSWORD),
        ])
        .await;
    assert_eq!(python_status(&results, "reset"), 200, "{results}");
    assert_eq!(python_status(&results, "login"), 200, "{results}");
    let fresh = python_token(&results, "login");
    assert!(issued_at(&fresh) > timestamp(&offboarded["authentication_revoked_at"]));
    let (status, body) = send(&world.served, "GET", "/api/auth/me", Some(&fresh), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    stale("after the new password", &world).await;
    let returned = world.account(SUBJECT).await;
    assert_eq!(returned["records"], 1, "the offboarding record stays");
    assert_eq!(returned["rows_with_email"], 1);
    assert_ne!(
        returned["kyc_status"], "APPROVED",
        "new KYC is still required"
    );
}

// ---------------------------------------------------------------------------
// 3 and 4. Rejected organization: unmarked and marked members.
// ---------------------------------------------------------------------------

/// Reject the person's organization through the served administrator route:
/// the organization is rejected, both members are detached and neither is
/// marked.
async fn reject_organization(world: &mut World) {
    let (status, body) = send(
        &world.served,
        "PUT",
        &format!("/api/auth/organization/{ORGANIZATION}/reject"),
        Some(&token(ADMIN)),
        Some(json!({"reason": "Company verification failed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let verification: String =
        sqlx::query_scalar("SELECT verification_status::text FROM organizations WHERE id=$1")
            .bind(ORGANIZATION)
            .fetch_one(&mut world.admin)
            .await
            .unwrap();
    assert_eq!(verification, "REJECTED");
    for member in [SUBJECT, COLLEAGUE] {
        let account = world.account(member).await;
        assert!(account["organization_id"].is_null(), "{account}");
        assert!(account["offboarded_at"].is_null(), "{account}");
    }
}

/// Rejection without offboarding keeps the owner's cleanup.
///
/// - A rejected user who is not offboarded (the colleague, still attached)
///   settles the confirmed sale in Rust, delivery and payment, and withdraws
///   an open order through Python's owner-cleanup authentication. Python
///   settlement requires an approved account, as before.
/// - A member detached by the rejection of the recorded organization (the
///   person) still reads the confirmed trade and settles it in Rust, through
///   the organization-less branch of the recorded-member rule. Python
///   refuses settlement without a current organization, so that seam is Rust
///   only.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn unmarked_rejection_keeps_settlement() {
    let mut world = World::new().await;
    world.market().await;
    sqlx::query("SELECT offboarding_market_order($1,$2,'ASK',1,1,'OPEN',NULL)")
        .bind(COLLEAGUE_ASK)
        .bind(COLLEAGUE)
        .execute(&mut world.admin)
        .await
        .unwrap();
    let (status, body) = send(
        &world.served,
        "PUT",
        &format!("/api/auth/reject/{COLLEAGUE}"),
        Some(&token(ADMIN)),
        Some(json!({"reason": "Account review"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rejected = world.account(COLLEAGUE).await;
    assert_eq!(rejected["status"], "REJECTED");
    assert_eq!(rejected["organization_id"], json!(ORGANIZATION));
    assert!(rejected["offboarded_at"].is_null());
    let colleague = token(COLLEAGUE);
    let (status, body) = send(
        &world.served,
        "PUT",
        &format!("/api/trades/{COLLEAGUE_CONFIRMED}/deliver"),
        Some(&colleague),
        Some(deliver_body()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "rejected user, Rust delivery: {body}"
    );
    let (status, body) = send(
        &world.served,
        "POST",
        &format!("/api/trades/{COLLEAGUE_CONFIRMED}/pay"),
        Some(&colleague),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "rejected user, Rust payment: {body}"
    );
    let results = world
        .python_now(vec![json!({
            "name": "withdraw", "method": "DELETE",
            "path": format!("/api/orderbook/{COLLEAGUE_ASK}"), "token": token(COLLEAGUE),
        })])
        .await;
    assert!(
        (200..300).contains(&python_status(&results, "withdraw")),
        "rejected user, Python withdrawal: {results}"
    );
    let withdrawn: String =
        sqlx::query_scalar("SELECT status::text FROM orderbook_orders WHERE id=$1")
            .bind(COLLEAGUE_ASK)
            .fetch_one(&mut world.admin)
            .await
            .unwrap();
    assert_eq!(withdrawn, "CANCELLED");
    assert_eq!(world.trade(COLLEAGUE_CONFIRMED).await["status"], "PAID");

    reject_organization(&mut world).await;
    let person = token(SUBJECT);
    let (status, body) = send(&world.served, "GET", "/api/trades/my", Some(&person), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains(&CONFIRMED.to_string()), "{body}");
    let (status, body) = send(
        &world.served,
        "PUT",
        &format!("/api/trades/{CONFIRMED}/deliver"),
        Some(&person),
        Some(deliver_body()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "detached member, Rust delivery: {body}"
    );
    let (status, body) = send(
        &world.served,
        "POST",
        &format!("/api/trades/{CONFIRMED}/pay"),
        Some(&person),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "detached member, Rust payment: {body}"
    );
    assert_eq!(world.trade(CONFIRMED).await["status"], "PAID");
}

/// The organization-less branch of the recorded-member rule and the trade
/// read admit a member of a rejected organization. An offboarded member of
/// that organization is refused on both, with an old token and with one
/// issued after the cutoff: only the marker refuses the second. Python issues
/// no new credential. The obligation stays for the company. That only
/// organization rejection and offboarding detach a member is checked in
/// `offboarding_acceptance.rs`.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn offboarded_detached_user_cannot_read_or_settle_rejected_org_trades() {
    let mut world = World::new().await;
    world.market().await;
    reject_organization(&mut world).await;
    let old = token(SUBJECT);
    let (status, body) = send(&world.served, "GET", "/api/trades/my", Some(&old), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains(&CONFIRMED.to_string()), "{body}");

    let (status, body) = world.offboard(SUBJECT, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["former_organization_id"], Value::Null);
    let account = assert_offboarded(&mut world, SUBJECT, "detached").await;
    let cutoff = timestamp(&account["authentication_revoked_at"]);
    world.wait_past(cutoff).await;
    let later = token(SUBJECT);
    for (name, bearer) in [("old", &old), ("after the cutoff", &later)] {
        let (status, body) = send(&world.served, "GET", "/api/trades/my", Some(bearer), None).await;
        assert!(refused(status), "trade read, {name}: {status} {body}");
        let (status, body) = send(
            &world.served,
            "PUT",
            &format!("/api/trades/{CONFIRMED}/deliver"),
            Some(bearer),
            Some(deliver_body()),
        )
        .await;
        assert!(refused(status), "settlement, {name}: {status} {body}");
    }
    let email = world.email(SUBJECT).await;
    let results = world
        .python_now(vec![
            python_trade("deliver", CONFIRMED, "deliver", &old),
            python_login("login", &email, PASSWORD),
        ])
        .await;
    assert!(python_status(&results, "deliver") >= 400, "{results}");
    assert!(
        matches!(python_status(&results, "login"), 401 | 403),
        "{results}"
    );
    let trade = world.trade(CONFIRMED).await;
    assert_eq!(trade["status"], "CONFIRMED");
    assert_eq!(trade["seller_user_id"], json!(SUBJECT));
}

// ---------------------------------------------------------------------------
// 5. Python session writers as lock holders.
// ---------------------------------------------------------------------------

/// Python login and Python refresh race the served command; each wins once.
///
/// When Python holds the person's row first, it commits its session and
/// tokens; the command then takes the cutoff after the lock, later than the
/// Python token's issuance, and revokes the new family: neither stack accepts
/// the new token or refresh session. When the command holds the row first,
/// the Python request waits, then finds the account rejected or its password
/// replaced and issues nothing.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn python_login_and_refresh_lock_holders_are_cut_off() {
    for flow in ["login", "refresh"] {
        for first in [First::Python, First::Rust] {
            let case = format!("{flow}, {first:?} first");
            let mut world = World::new().await;
            let email = world.email(SUBJECT).await;
            let request = if flow == "login" {
                python_login("raced", &email, PASSWORD)
            } else {
                let results = world
                    .python_now(vec![python_login("login", &email, PASSWORD)])
                    .await;
                assert_eq!(python_status(&results, "login"), 200, "{case}: {results}");
                python_session(
                    "raced",
                    "/api/auth/refresh",
                    &python_cookies(&results, "login"),
                )
            };
            let sessions_before = world.account(SUBJECT).await["sessions"].clone();
            let outcome = race(
                &mut world,
                first,
                "session-insert",
                SUBJECT,
                Some(ORGANIZATION),
                vec![request],
            )
            .await;
            assert_eq!(
                outcome.command.0,
                StatusCode::OK,
                "{case}: {:?}",
                outcome.command
            );
            let account = assert_offboarded(&mut world, SUBJECT, &case).await;
            let status = python_status(&outcome.python, "raced");
            match first {
                First::Python => {
                    assert_eq!(status, 200, "{case}: {}", outcome.python);
                    let issued = python_token(&outcome.python, "raced");
                    let cutoff = timestamp(&account["authentication_revoked_at"]);
                    assert!(
                        issued_at(&issued) < cutoff,
                        "{case}: the cutoff follows the lock holder's token"
                    );
                    assert!(timestamp(&account["password_changed_at"]) >= cutoff);
                    assert_token_refused(&world, &issued, &case).await;
                    let results = world
                        .python_now(vec![python_session(
                            "refresh",
                            "/api/auth/refresh",
                            &python_cookies(&outcome.python, "raced"),
                        )])
                        .await;
                    assert_eq!(python_status(&results, "refresh"), 401, "{case}: {results}");
                }
                First::Rust => {
                    assert!(
                        matches!(status, 401 | 403),
                        "{case}: issues nothing: {}",
                        outcome.python
                    );
                    assert_eq!(
                        account["sessions"], sessions_before,
                        "{case}: no new session row"
                    );
                }
            }
            eprintln!("{case}: Python answered {status}");
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Python approval and join approval.
// ---------------------------------------------------------------------------

/// Python approval and join approval race the served command; each wins once.
///
/// Approval: an attached, rejected but unmarked member. If Python approves
/// first, the command offboards the approved member after it. If the command
/// commits first, the approval meets the marker constraint and changes
/// nothing.
///
/// Join approval: a detached, approved person with a pending request to join
/// the organization, offboarded with an explicit null organization. If Python
/// attaches the person first, the command finds the membership changed and
/// refuses; a fresh command for the new organization then completes. If the
/// command commits first, it has rejected the pending request and the Python
/// approval refuses.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn python_approval_and_join_race_the_command() {
    for first in [First::Python, First::Rust] {
        let case = format!("approval, {first:?} first");
        let mut world = World::new().await;
        sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
            .bind(SUBJECT)
            .execute(&mut world.admin)
            .await
            .unwrap();
        let outcome = race(
            &mut world,
            first,
            "status-transition",
            SUBJECT,
            Some(ORGANIZATION),
            vec![python_approve("approve", SUBJECT)],
        )
        .await;
        assert_eq!(
            outcome.command.0,
            StatusCode::OK,
            "{case}: {:?}",
            outcome.command
        );
        assert_offboarded(&mut world, SUBJECT, &case).await;
        let status = python_status(&outcome.python, "approve");
        match first {
            First::Python => assert_eq!(status, 200, "{case}: {}", outcome.python),
            First::Rust => assert!(status >= 400, "{case}: {}", outcome.python),
        }
        eprintln!("{case}: Python approval answered {status}");
    }

    for first in [First::Python, First::Rust] {
        let case = format!("join approval, {first:?} first");
        let mut world = World::new().await;
        sqlx::query("SELECT offboarding_market_detach($1)")
            .bind(SUBJECT)
            .execute(&mut world.admin)
            .await
            .unwrap();
        world
            .call(
                "acceptance_join_request",
                &[json!(REJOIN_REQUEST), json!(SUBJECT), json!(ORGANIZATION)],
            )
            .await;
        let outcome = race(
            &mut world,
            first,
            "join-approval",
            SUBJECT,
            None,
            vec![python_join_approval("join", REJOIN_REQUEST)],
        )
        .await;
        let status = python_status(&outcome.python, "join");
        match first {
            First::Python => {
                assert_eq!(status, 200, "{case}: {}", outcome.python);
                assert_eq!(outcome.command.0, StatusCode::CONFLICT, "{case}");
                assert_eq!(
                    code(&outcome.command.1),
                    "OFFBOARD_MEMBERSHIP_CHANGED",
                    "{case}"
                );
                let attached = world.account(SUBJECT).await;
                assert_eq!(attached["organization_id"], json!(ORGANIZATION), "{case}");
                assert!(attached["offboarded_at"].is_null(), "{case}");
                let (status, body) = world.offboard(SUBJECT, Some(ORGANIZATION)).await;
                assert_eq!(status, StatusCode::OK, "{case}: {body}");
            }
            First::Rust => {
                assert_eq!(outcome.command.0, StatusCode::OK, "{case}");
                assert!(status >= 400, "{case}: {}", outcome.python);
                let account = world.account(SUBJECT).await;
                assert_eq!(
                    account["joins"][REJOIN_REQUEST.to_string()],
                    "REJECTED",
                    "{case}"
                );
            }
        }
        assert_offboarded(&mut world, SUBJECT, &case).await;
        eprintln!("{case}: Python join approval answered {status}");
    }
}

// ---------------------------------------------------------------------------
// 7. Logout, recovery and invitation.
// ---------------------------------------------------------------------------

/// Python logout, password recovery and invitation acceptance race the served
/// command; each wins once. Every order ends with no live session, no
/// recovery secret on the account and no token either stack accepts, except
/// one bounded case that the test records:
///
/// Python `forgot-password` reads the account without a lock. When the
/// command commits between that read and the write, Python stores a new
/// reset secret on the offboarded account and mails it. The secret grants no
/// access: Python's reset sets a password, but neither stack signs a rejected
/// or marked account in. Rust recovery refuses the marked account outright.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn logout_recovery_and_invitation_race_the_command() {
    // Logout.
    for first in [First::Python, First::Rust] {
        let case = format!("logout, {first:?} first");
        let mut world = World::new().await;
        let email = world.email(SUBJECT).await;
        let results = world
            .python_now(vec![python_login("login", &email, PASSWORD)])
            .await;
        assert_eq!(python_status(&results, "login"), 200, "{case}: {results}");
        let cookies = python_cookies(&results, "login");
        let issued = python_token(&results, "login");
        let outcome = race(
            &mut world,
            first,
            "session-revoke",
            SUBJECT,
            Some(ORGANIZATION),
            vec![python_session("logout", "/api/auth/logout", &cookies)],
        )
        .await;
        assert_eq!(
            outcome.command.0,
            StatusCode::OK,
            "{case}: {:?}",
            outcome.command
        );
        assert_eq!(
            python_status(&outcome.python, "logout"),
            200,
            "{case}: {}",
            outcome.python
        );
        assert_offboarded(&mut world, SUBJECT, &case).await;
        assert_token_refused(&world, &issued, &case).await;
    }

    // Recovery through Python forgot-password.
    for first in [First::Python, First::Rust] {
        let case = format!("recovery, {first:?} first");
        let mut world = World::new().await;
        let email = world.email(SUBJECT).await;
        let outcome = race(
            &mut world,
            first,
            "reset-secret",
            SUBJECT,
            Some(ORGANIZATION),
            vec![
                json!({"name": "forgot", "method": "POST", "path": "/api/auth/forgot-password",
                        "json": {"email": email}}),
            ],
        )
        .await;
        assert_eq!(
            outcome.command.0,
            StatusCode::OK,
            "{case}: {:?}",
            outcome.command
        );
        assert_eq!(python_status(&outcome.python, "forgot"), 200, "{case}");
        let account = world.account(SUBJECT).await;
        assert!(!account["offboarded_at"].is_null(), "{case}");
        match first {
            First::Python => {
                assert!(
                    account["password_reset_token_hash"].is_null(),
                    "{case}: the command cleared the secret Python stored first"
                );
            }
            First::Rust => {
                // The bounded residual: a secret stored after the commit.
                eprintln!(
                    "{case}: Python stored a reset secret after the commit: {}",
                    !account["password_reset_token_hash"].is_null()
                );
                // Whatever the secret, it grants no access. A known secret of
                // the same form stands in for the mailed one.
                let secret = format!("mailed-reset-{}", Uuid::new_v4());
                world
                    .call(
                        "acceptance_reset_secret",
                        &[json!(SUBJECT), json!(sha256_hex(&secret))],
                    )
                    .await;
                let results = world
                    .python_now(vec![
                        json!({"name": "reset", "method": "POST", "path": "/api/auth/reset-password",
                               "json": {"token": secret, "new_password": NEW_PASSWORD}}),
                        python_login("login", &email, NEW_PASSWORD),
                    ])
                    .await;
                eprintln!(
                    "{case}: Python reset answered {}",
                    python_status(&results, "reset")
                );
                assert!(
                    matches!(python_status(&results, "login"), 401 | 403),
                    "{case}: {results}"
                );
                let tokens_before = world.account(SUBJECT).await["reset_tokens"].clone();
                let rust_forgot = Request::builder()
                    .method("POST")
                    .uri("/api/auth/forgot-password")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"email": email}).to_string()))
                    .unwrap();
                let response = world.served.clone().oneshot(rust_forgot).await.unwrap();
                eprintln!(
                    "{case}: Rust forgot-password answered {}",
                    response.status()
                );
                assert_eq!(
                    world.account(SUBJECT).await["reset_tokens"],
                    tokens_before,
                    "{case}: Rust stores no secret for a marked account"
                );
                assert!(!world.account(SUBJECT).await["offboarded_at"].is_null());
            }
        }
    }

    // Invitation acceptance.
    for first in [First::Python, First::Rust] {
        let case = format!("invitation, {first:?} first");
        let mut world = World::new().await;
        let invitee_email = format!("invitee-{}@northwind-fuels.com", Uuid::new_v4().simple());
        let results = world
            .python_now(vec![json!({
                "name": "invite", "method": "POST", "path": "/api/auth/admin/invitations",
                "token": token(ADMIN),
                "json": {"email": invitee_email, "first_name": "Invited", "role": "SUPPLIER",
                         "organization_id": ORGANIZATION},
            })])
            .await;
        assert_eq!(python_status(&results, "invite"), 201, "{case}: {results}");
        let invitee: Uuid = results["invite"]["body"]["user_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let link = results["invite"]["body"]["acceptance_url"]
            .as_str()
            .unwrap();
        let secret = link.split_once("#token=").unwrap().1.to_owned();
        let outcome = race(
            &mut world,
            first,
            "password-set",
            invitee,
            Some(ORGANIZATION),
            vec![json!({"name": "accept", "method": "POST", "path": "/api/auth/invitations/accept",
                        "json": {"token": secret, "new_password": NEW_PASSWORD, "accept_terms": true}})],
        )
        .await;
        assert_eq!(
            outcome.command.0,
            StatusCode::OK,
            "{case}: {:?}",
            outcome.command
        );
        assert_offboarded(&mut world, invitee, &case).await;
        let status = python_status(&outcome.python, "accept");
        match first {
            First::Python => {
                assert_eq!(status, 200, "{case}: {}", outcome.python);
                let issued = python_token(&outcome.python, "accept");
                assert_token_refused(&world, &issued, &case).await;
            }
            First::Rust => assert_eq!(status, 400, "{case}: {}", outcome.python),
        }
        let results = world
            .python_now(vec![python_login("login", &invitee_email, NEW_PASSWORD)])
            .await;
        assert!(
            matches!(python_status(&results, "login"), 401 | 403),
            "{case}: {results}"
        );
        eprintln!("{case}: Python acceptance answered {status}");
    }
}

// ---------------------------------------------------------------------------
// 8. Python settlement and the responsible-person reassignment.
// ---------------------------------------------------------------------------

/// After the seller is offboarded, a Python delivery by the buyer and the
/// reassignment of the seller side to a colleague race on the same trade;
/// each wins once. Python settlement locks the trade before people and
/// organizations, the reassignment the other way round, so a reassignment
/// that waits past its 500 ms bound answers `MARKET_BUSY` with nothing kept
/// and is repeated. Either order ends delivered, with the colleague as the
/// seller, the terms unchanged, and payment by the colleague accepted in
/// Python, the new person rule.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL and Python"]
async fn python_settlement_races_reassignment() {
    for first in [First::Python, First::Rust] {
        let case = format!("settlement, {first:?} first");
        let mut world = World::new().await;
        world.market().await;
        let (status, body) = world.offboard(SUBJECT, Some(ORGANIZATION)).await;
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        let before = world.trade(CONFIRMED).await;
        let reassign = json!({
            "side": "SELLER", "replacement_user_id": COLLEAGUE,
            "evidence_case_ref": CASE, "expected_current_user_id": SUBJECT,
        });
        let path = format!("/api/auth/admin/trades/{CONFIRMED}/reassign-responsible");
        let python = world
            .python(vec![python_trade(
                "deliver",
                CONFIRMED,
                "deliver",
                &token(COUNTERPARTY),
            )])
            .await;
        let gate = match first {
            First::Python => "trade-settlement",
            First::Rust => "trade-reassignment",
        };
        world.call("acceptance_gate_install", &[json!(gate)]).await;
        let mut keeper = PgConnection::connect(&world.database.admin_url)
            .await
            .unwrap();
        sqlx::query("SELECT acceptance_gate_close()")
            .execute(&mut keeper)
            .await
            .unwrap();
        let start_reassignment = |served: Router, path: String, body: Value| {
            tokio::spawn(async move {
                send(&served, "POST", &path, Some(&token(ADMIN)), Some(body)).await
            })
        };
        let (python, reassigned) = match first {
            First::Python => {
                let python = python.go();
                let holder = gate_waiter(&world.observer).await;
                let reassigned =
                    start_reassignment(world.served.clone(), path.clone(), reassign.clone());
                let (waiter, application) = blocked_by(&world.observer, holder).await;
                assert_eq!(application, RUST_APPLICATION, "{case}");
                eprintln!("{case}: Rust backend {waiter} waited for Python backend {holder}");
                (python, reassigned)
            }
            First::Rust => {
                let reassigned =
                    start_reassignment(world.served.clone(), path.clone(), reassign.clone());
                let holder = gate_waiter(&world.observer).await;
                let python = python.go();
                let (waiter, _) = blocked_by(&world.observer, holder).await;
                eprintln!("{case}: Python backend {waiter} waited for Rust backend {holder}");
                (python, reassigned)
            }
        };
        sqlx::query("SELECT acceptance_gate_open()")
            .execute(&mut keeper)
            .await
            .unwrap();
        keeper.close().await.unwrap();
        let python = python.await.unwrap();
        let (mut status, mut body) = reassigned.await.unwrap();
        world.call("acceptance_gate_remove", &[]).await;
        assert_eq!(python_status(&python, "deliver"), 200, "{case}: {python}");
        if status == StatusCode::CONFLICT && code(&body) == "MARKET_BUSY" {
            assert_eq!(
                world.trade(CONFIRMED).await["seller_user_id"],
                json!(SUBJECT),
                "{case}: busy kept nothing"
            );
            eprintln!("{case}: the reassignment waited past its lock bound; repeated");
            (status, body) = send(
                &world.served,
                "POST",
                &path,
                Some(&token(ADMIN)),
                Some(reassign),
            )
            .await;
        }
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        let results = world
            .python_now(vec![
                python_trade("departed pays", CONFIRMED, "pay", &token(SUBJECT)),
                python_trade("colleague pays", CONFIRMED, "pay", &token(COLLEAGUE)),
            ])
            .await;
        assert!(
            python_status(&results, "departed pays") >= 400,
            "{case}: {results}"
        );
        assert_eq!(
            python_status(&results, "colleague pays"),
            200,
            "{case}: {results}"
        );
        let after = world.trade(CONFIRMED).await;
        assert_eq!(after["status"], "PAID", "{case}");
        assert_eq!(after["seller_user_id"], json!(COLLEAGUE), "{case}");
        for field in [
            "quantity_mt",
            "price_per_mt_usd",
            "buyer_id",
            "seller_id",
            "buyer_user_id",
        ] {
            assert_eq!(after[field], before[field], "{case}: {field}");
        }
    }
}

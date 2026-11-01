//! PostgreSQL and cross-stack checks of the account credential cutoff.
//!
//! These cases need a migrated disposable database and the project's Python
//! interpreter (`RUST_TEST_PYTHON`, default `python3`):
//!
//! - `OFFBOARDING_TEST_DATABASE_URL`: the runtime application role;
//! - `OFFBOARDING_TEST_OWNER_URL`: the schema owner, used only for fixtures.
//!
//! Run them with `--include-ignored --test-threads=1`. The Python half runs the
//! unchanged Python application code through
//! `tests/fixtures/offboarding_python_harness.py` against the same database.
#![cfg(feature = "contract-tests")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool, Row, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{
    AccountState, AuthError, TokenIssuer, TokenType, VerificationSettings, cutoff_refusal,
    limits::SharedLimits,
    login,
    offboarding::{CUTOFF_SKEW_MARGIN_SECONDS, CredentialRevocation, revoke_account_credentials},
    password, refresh,
    store::{self, NewRefreshRow},
    token_bytes::{generate_token, token_digest},
    validate_account, verify_access, verify_token,
};

const SECRET: &str = "synthetic-offboarding-key-at-least-32-characters";
const ISSUER: &str = "verdaxis";
const AUDIENCE: &str = "verdaxis-api";
const ORIGIN: &str = "https://app.verdaxis.exchange";
const PASSWORD: &str = "SyntheticPassword123";
/// Every pool waits at most this long for a row lock, as a request would.
const LOCK_TIMEOUT: &str = "500ms";
/// How long a schedule keeps a lock after the other side is seen waiting. Any
/// positive wait separates the database clock after the lock from the clock
/// before it; a short hold keeps the whole wait well under [`LOCK_TIMEOUT`].
const HOLD: Duration = Duration::from_millis(20);
/// bcrypt's minimum cost keeps fixture password checks short while a row
/// lock is held; verification accepts any cost.
const FIXTURE_HASH_COST: u32 = 4;
/// Python private streams recheck authorization every 15 seconds.
const STREAM_RECHECK: Duration = Duration::from_secs(15);
/// Allowance for the streams' own loop timeouts and a loaded test host.
const STREAM_SLACK: Duration = Duration::from_secs(2);
const HARNESS: &str = "rs/read/tests/fixtures/offboarding_python_harness.py";
const RESULT_PREFIX: &str = "OFFBOARDING_RESULT ";

// ---------------------------------------------------------------------------
// Database fixture.
// ---------------------------------------------------------------------------

struct Database {
    owner: PgPool,
    /// Runtime role: login, refresh and every observation of lock waits.
    session: PgPool,
    /// Runtime role: the transaction that takes the credential cutoff.
    cutoff: PgPool,
    cutoff_tag: String,
    session_tag: String,
    runtime_url: String,
    owner_url: String,
}

struct TestUser {
    id: Uuid,
    email: String,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must name a disposable database"))
}

async fn tagged_pool(url: &str, tag: &str, connections: u32) -> PgPool {
    let tag = tag.to_owned();
    PgPoolOptions::new()
        .max_connections(connections)
        .after_connect(move |connection, _| {
            let tag = tag.clone();
            Box::pin(async move {
                sqlx::query(
                    "SELECT set_config('application_name', $1, false), \
                     set_config('lock_timeout', $2, false)",
                )
                .bind(tag)
                .bind(LOCK_TIMEOUT)
                .execute(connection)
                .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("disposable database")
}

impl Database {
    async fn connect() -> Self {
        let tag = format!("offboarding-{}", Uuid::new_v4().simple());
        let runtime_url = required("OFFBOARDING_TEST_DATABASE_URL");
        let owner_url = required("OFFBOARDING_TEST_OWNER_URL");
        let session_tag = format!("{tag}-session");
        let cutoff_tag = format!("{tag}-cutoff");
        let owner = tagged_pool(&owner_url, &format!("{tag}-owner"), 4).await;
        let session = tagged_pool(&runtime_url, &session_tag, 4).await;
        let cutoff = tagged_pool(&runtime_url, &cutoff_tag, 2).await;
        // Open every connection before any race, so no lock wait includes the
        // time to open a connection.
        for (pool, connections) in [(&owner, 4), (&session, 4), (&cutoff, 2)] {
            warm(pool, connections).await;
        }
        Self {
            owner,
            session,
            cutoff,
            cutoff_tag,
            session_tag,
            runtime_url,
            owner_url,
        }
    }

    async fn organization(&self) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations (id, name, type, verification_status, provenance) \
             VALUES ($1, $2, 'FUEL_BUYER', 'APPROVED', 'REAL')",
        )
        .bind(id)
        .bind(format!("Synthetic offboarding company {id}"))
        .execute(&self.owner)
        .await
        .unwrap();
        id
    }

    async fn user(&self, organization: Option<Uuid>) -> TestUser {
        let id = Uuid::new_v4();
        let email = format!("offboarding-{id}@example.com");
        let hash = bcrypt::hash(PASSWORD, FIXTURE_HASH_COST).unwrap();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, status, organization_id, \
             email_verified, must_change_password) \
             VALUES ($1, $2, $3, 'BUYER', 'APPROVED', $4, true, false)",
        )
        .bind(id)
        .bind(&email)
        .bind(hash)
        .bind(organization)
        .execute(&self.owner)
        .await
        .unwrap();
        TestUser { id, email }
    }

    async fn live_families(&self, user: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM refresh_sessions \
             WHERE user_id = $1 AND NOT revoked AND expires_at > now()",
        )
        .bind(user)
        .fetch_one(&self.owner)
        .await
        .unwrap()
    }

    /// The account change the offboarding command makes besides credentials.
    async fn detach(&self, user: Uuid) {
        sqlx::query("UPDATE users SET status = 'REJECTED', organization_id = NULL WHERE id = $1")
            .bind(user)
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

async fn warm(pool: &PgPool, connections: usize) {
    let mut open = Vec::with_capacity(connections);
    for _ in 0..connections {
        open.push(pool.acquire().await.unwrap());
    }
}

async fn lock_user_row(connection: &mut PgConnection, user: Uuid) {
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .fetch_one(connection)
        .await
        .unwrap();
}

async fn database_now(connection: &mut PgConnection) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(connection)
        .await
        .unwrap()
}

/// One complete caller transaction: lock the user row, then cut off.
async fn cut_off(pool: &PgPool, user: Uuid) -> CredentialRevocation {
    let mut transaction = pool.begin().await.unwrap();
    lock_user_row(&mut transaction, user).await;
    let revocation = revoke_account_credentials(&mut transaction, user)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    revocation
}

/// Wait until a backend of `application_name` waits for a heavyweight lock.
/// `observer` must use the same database role, which may read that state.
async fn wait_for_lock_wait(observer: &PgPool, application_name: &str) {
    let started = Instant::now();
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
             WHERE datname = current_database() AND application_name = $1 \
             AND wait_event_type = 'Lock')",
        )
        .bind(application_name)
        .fetch_one(observer)
        .await
        .unwrap();
        if waiting {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{application_name} never waited for the user row lock"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---------------------------------------------------------------------------
// Tokens and both stacks' checks.
// ---------------------------------------------------------------------------

fn verification() -> VerificationSettings {
    VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into())
}

fn issuer() -> TokenIssuer {
    TokenIssuer::new(SECRET, ISSUER, AUDIENCE)
}

fn issued_at(token: &str) -> DateTime<Utc> {
    let payload = token.split('.').nth(1).expect("JWT payload");
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    DateTime::from_timestamp_micros(claims["iat_us"].as_i64().expect("iat_us")).unwrap()
}

/// An access token with the given issuance claims and a valid expiry.
fn access_token(user: Uuid, iat_us: Option<Value>, iat: Option<Value>) -> String {
    let mut claims = json!({
        "sub": user.to_string(),
        "type": "access",
        "exp": (Utc::now() + TimeDelta::minutes(15)).timestamp(),
        "iss": ISSUER,
        "aud": AUDIENCE,
    });
    if let Some(value) = iat_us {
        claims["iat_us"] = value;
    }
    if let Some(value) = iat {
        claims["iat"] = value;
    }
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

async fn account_state(pool: &PgPool, user: Uuid) -> AccountState {
    let row = sqlx::query(
        "SELECT status::text AS status, must_change_password, password_changed_at, \
         authentication_revoked_at, offboarded_at FROM users WHERE id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap();
    let password: Option<DateTime<Utc>> = row.get("password_changed_at");
    let revoked: Option<DateTime<Utc>> = row.get("authentication_revoked_at");
    let offboarded: Option<DateTime<Utc>> = row.get("offboarded_at");
    AccountState {
        exists: true,
        status: row.get("status"),
        must_change_password: row.get("must_change_password"),
        password_changed_at: password.map(|value| value.timestamp_micros()),
        authentication_revoked_at: revoked.map(|value| value.timestamp_micros()),
        offboarded: offboarded.is_some(),
    }
}

/// Rust bearer validation, as every ordinary authenticated route applies it.
async fn rust_normal(pool: &PgPool, token: &str) -> Result<(), AuthError> {
    let access = verify_access(token, &verification(), Utc::now())?;
    validate_account(&access, &account_state(pool, access.subject).await)
}

/// The owner-cleanup rule: a rejected account keeps settlement access, so
/// only the password cutoff applies there, never the revocation cutoff.
async fn rust_cleanup(pool: &PgPool, token: &str) -> Result<(), AuthError> {
    let verified = verify_token(token, &verification(), TokenType::Access, Utc::now())?;
    let mut account = account_state(pool, verified.subject()).await;
    account.authentication_revoked_at = None;
    cutoff_refusal(verified.issuance(), &account).map_or(Ok(()), Err)
}

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

fn python_command(command: &str, database_url: &str) -> Command {
    let root = repository_root();
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut process = Command::new(python);
    process
        .current_dir(&root)
        .env("PYTHONPATH", &root)
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", SECRET)
        .env("JWT_ISSUER", ISSUER)
        .env("JWT_AUDIENCE", AUDIENCE)
        .env("DATABASE_URL", python_url(database_url))
        .arg(root.join(HARNESS))
        .arg(command);
    process
}

/// Run one harness command and return its JSON result.
async fn python(command: &'static str, database_url: &str, request: Value) -> Value {
    let mut process = python_command(command, database_url);
    tokio::task::spawn_blocking(move || {
        let mut child = process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Python interpreter unavailable: set RUST_TEST_PYTHON or put python3 on PATH");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(request.to_string().as_bytes())
            .expect("write request");
        let output = child.wait_with_output().expect("Python harness finished");
        assert!(
            output.status.success(),
            "Python harness {command} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
        let line = stdout
            .lines()
            .find_map(|line| line.strip_prefix(RESULT_PREFIX))
            .expect("one result line");
        serde_json::from_str(line).expect("JSON result")
    })
    .await
    .unwrap()
}

/// Python's normal and owner-cleanup authentication for named tokens.
async fn python_auth(db: &Database, tokens: &[(String, String)]) -> Value {
    let tokens: Vec<Value> = tokens
        .iter()
        .map(|(name, token)| json!({"name": name, "token": token}))
        .collect();
    python("auth", &db.runtime_url, json!({"tokens": tokens})).await["tokens"].clone()
}

fn python_refused(outcome: &Value) -> bool {
    let outcome = outcome.as_str().expect("outcome text");
    outcome.starts_with("refused:") || outcome.starts_with("error:")
}

// ---------------------------------------------------------------------------
// Rust login and refresh through their routers.
// ---------------------------------------------------------------------------

/// The serving composition: the login's unknown-account hash is prepared
/// before any request, so no attempt computes it while holding a row lock.
async fn login_state(pool: &PgPool, held: Option<Arc<Barrier>>) -> login::LoginState {
    let mut state = login::LoginState::new(
        pool.clone(),
        "production",
        SECRET,
        None,
        ISSUER,
        AUDIENCE,
        SharedLimits::default(),
    )
    .await;
    state.hooks.user_locked = held;
    state
}

fn refresh_state(pool: &PgPool, held: Option<Arc<Barrier>>) -> refresh::RefreshState {
    let mut state = refresh::RefreshState::new(
        pool.clone(),
        "production",
        SECRET,
        None,
        ISSUER,
        AUDIENCE,
        SharedLimits::default(),
    );
    state.hooks.user_locked = held;
    state
}

struct Issued {
    status: u16,
    body: Value,
    refresh_cookie: Option<String>,
}

async fn issued(response: axum::response::Response) -> Issued {
    let status = response.status().as_u16();
    let refresh_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| cookie.strip_prefix("refresh_token="))
        .map(|rest| rest.split(';').next().unwrap_or_default().to_owned());
    let bytes = to_bytes(response.into_body(), 16384).await.unwrap();
    Issued {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        refresh_cookie,
    }
}

async fn send_login(state: login::LoginState, email: String, device: String) -> Issued {
    let response = login::router(state)
        .oneshot(
            Request::post("/api/auth/login")
                .header("origin", ORIGIN)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("device_session={device}"))
                .body(Body::from(format!(
                    "username={}&password={PASSWORD}",
                    email.replace('@', "%40")
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    issued(response).await
}

async fn send_refresh(state: refresh::RefreshState, token: String, device: String) -> Issued {
    let response = refresh::router(state)
        .oneshot(
            Request::post("/api/auth/refresh")
                .header("origin", ORIGIN)
                .header(
                    "cookie",
                    format!("refresh_token={token}; device_session={device}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    issued(response).await
}

/// One live device-bound refresh family and its presented token.
async fn seed_family(db: &Database, user: Uuid) -> (String, String) {
    let device = generate_token();
    let jti = generate_token();
    let family = Uuid::new_v4();
    let now = Utc::now() - TimeDelta::seconds(1);
    store::insert_refresh(
        &mut db.owner.acquire().await.unwrap(),
        &NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + TimeDelta::days(7),
        },
    )
    .await
    .unwrap();
    (issuer().refresh(user, family, &jti, now, None), device)
}

fn error_code(issued: &Issued) -> &str {
    issued.body["detail"]["code"].as_str().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Lock races.
// ---------------------------------------------------------------------------

/// The caller side of a race: start the transaction, wait for the user row
/// lock, read the database clock once it is granted, then cut off.
async fn cut_off_after_wait(pool: PgPool, user: Uuid) -> (DateTime<Utc>, CredentialRevocation) {
    let mut transaction = pool.begin().await.unwrap();
    let requested = Instant::now();
    lock_user_row(&mut transaction, user).await;
    println!(
        "the cutoff transaction waited {:?} for the user row",
        requested.elapsed()
    );
    let acquired = database_now(&mut transaction).await;
    let revocation = revoke_account_credentials(&mut transaction, user)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    (acquired, revocation)
}

/// The cutoff is read from the database clock after the lock is granted, plus
/// exactly the documented margin, and so lies after every token the lock
/// holder issued.
fn assert_cutoff_after_lock(
    acquired: DateTime<Utc>,
    revocation: &CredentialRevocation,
    issued_while_waiting: DateTime<Utc>,
) {
    let margin = TimeDelta::seconds(CUTOFF_SKEW_MARGIN_SECONDS);
    assert!(
        revocation.cutoff >= acquired + margin,
        "the cutoff {} must be read after the lock was granted at {acquired}",
        revocation.cutoff
    );
    assert!(
        revocation.cutoff <= acquired + margin + TimeDelta::seconds(2),
        "the cutoff adds only the documented margin"
    );
    assert!(
        revocation.cutoff - margin > issued_while_waiting,
        "a token issued by the lock holder must be before the unmargined cutoff"
    );
    assert_eq!(revocation.authentication_revoked_at, revocation.cutoff);
    assert_eq!(revocation.password_changed_at, revocation.cutoff);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn cutoff_is_taken_after_user_lock() {
    let db = Database::connect().await;
    let mut python_cases: Vec<(String, String)> = Vec::new();

    // Login holds the user row lock; the cutoff transaction waits for it.
    let user = db.user(None).await;
    let device = generate_token();
    let held = Arc::new(Barrier::new(2));
    let attempt = tokio::spawn(send_login(
        login_state(&db.session, Some(held.clone())).await,
        user.email.clone(),
        device.clone(),
    ));
    held.wait().await;
    let cutoff = tokio::spawn(cut_off_after_wait(db.cutoff.clone(), user.id));
    wait_for_lock_wait(&db.session, &db.cutoff_tag).await;
    tokio::time::sleep(HOLD).await;
    held.wait().await;
    let login = attempt.await.unwrap();
    assert_eq!(login.status, 200, "the lock holder signs in first");
    let access = login.body["access_token"].as_str().unwrap().to_owned();
    let (acquired, revocation) = cutoff.await.unwrap();
    assert_cutoff_after_lock(acquired, &revocation, issued_at(&access));
    assert_eq!(db.live_families(user.id).await, 0);
    assert!(matches!(
        rust_normal(&db.owner, &access).await,
        Err(AuthError::PasswordChanged)
    ));
    assert!(matches!(
        rust_cleanup(&db.owner, &access).await,
        Err(AuthError::PasswordChanged)
    ));
    let replayed = send_refresh(
        refresh_state(&db.session, None),
        login.refresh_cookie.expect("login refresh cookie"),
        device,
    )
    .await;
    assert_eq!(
        replayed.status, 401,
        "the login's refresh family is revoked"
    );
    python_cases.push(("login_during_wait".into(), access));

    // Refresh holds the user row lock; the cutoff transaction waits for it.
    let user = db.user(None).await;
    let (token, device) = seed_family(&db, user.id).await;
    let held = Arc::new(Barrier::new(2));
    let attempt = tokio::spawn(send_refresh(
        refresh_state(&db.session, Some(held.clone())),
        token,
        device.clone(),
    ));
    held.wait().await;
    let cutoff = tokio::spawn(cut_off_after_wait(db.cutoff.clone(), user.id));
    wait_for_lock_wait(&db.session, &db.cutoff_tag).await;
    tokio::time::sleep(HOLD).await;
    held.wait().await;
    let rotated = attempt.await.unwrap();
    assert_eq!(rotated.status, 200, "the lock holder refreshes first");
    let access = rotated.body["access_token"].as_str().unwrap().to_owned();
    let (acquired, revocation) = cutoff.await.unwrap();
    assert_cutoff_after_lock(acquired, &revocation, issued_at(&access));
    assert_eq!(db.live_families(user.id).await, 0);
    assert!(matches!(
        rust_normal(&db.owner, &access).await,
        Err(AuthError::PasswordChanged)
    ));
    let successor = send_refresh(
        refresh_state(&db.session, None),
        rotated.refresh_cookie.expect("rotated refresh cookie"),
        device,
    )
    .await;
    assert_eq!(
        successor.status, 401,
        "the rotated successor is revoked too"
    );
    python_cases.push(("refresh_during_wait".into(), access));

    // The cutoff transaction holds the lock; a login waits and then fails.
    let user = db.user(None).await;
    let mut transaction = db.cutoff.begin().await.unwrap();
    lock_user_row(&mut transaction, user.id).await;
    revoke_account_credentials(&mut transaction, user.id)
        .await
        .unwrap();
    let attempt = tokio::spawn(send_login(
        login_state(&db.session, None).await,
        user.email.clone(),
        generate_token(),
    ));
    wait_for_lock_wait(&db.session, &db.session_tag).await;
    tokio::time::sleep(HOLD).await;
    transaction.commit().await.unwrap();
    let refused = attempt.await.unwrap();
    assert_eq!(refused.status, 401);
    assert_eq!(error_code(&refused), "AUTH_CREDENTIALS_INVALID");
    assert_eq!(db.live_families(user.id).await, 0, "no session was issued");

    // The cutoff transaction holds the lock; a refresh waits and then fails.
    let user = db.user(None).await;
    let (token, device) = seed_family(&db, user.id).await;
    let mut transaction = db.cutoff.begin().await.unwrap();
    lock_user_row(&mut transaction, user.id).await;
    revoke_account_credentials(&mut transaction, user.id)
        .await
        .unwrap();
    let attempt = tokio::spawn(send_refresh(
        refresh_state(&db.session, None),
        token,
        device,
    ));
    wait_for_lock_wait(&db.session, &db.session_tag).await;
    tokio::time::sleep(HOLD).await;
    transaction.commit().await.unwrap();
    let refused = attempt.await.unwrap();
    assert_eq!(refused.status, 401);
    assert_eq!(error_code(&refused), "REFRESH_PASSWORD_CHANGED");
    assert_eq!(db.live_families(user.id).await, 0, "no session was issued");

    // Python refuses both tokens issued by a lock holder, on normal and
    // owner-cleanup routes.
    let outcomes = python_auth(&db, &python_cases).await;
    for (name, _) in &python_cases {
        for route in ["normal", "cleanup"] {
            assert_eq!(
                outcomes[name][route], "refused:401:AUTH_PASSWORD_CHANGED",
                "Python {route} route accepted the {name} token"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cutoff values.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn cutoffs_never_move_back() {
    let db = Database::connect().await;
    let later = Utc::now() + TimeDelta::hours(1);
    let earlier = Utc::now() - TimeDelta::days(1);
    let ahead = db.user(None).await;
    let behind = db.user(None).await;
    sqlx::query(
        "UPDATE users SET authentication_revoked_at = $2, password_changed_at = NULL WHERE id = $1",
    )
    .bind(ahead.id)
    .bind(later)
    .execute(&db.owner)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE users SET authentication_revoked_at = $2, password_changed_at = $2 WHERE id = $1",
    )
    .bind(behind.id)
    .bind(earlier)
    .execute(&db.owner)
    .await
    .unwrap();

    let first = cut_off(&db.cutoff, ahead.id).await;
    assert_eq!(
        first.authentication_revoked_at.timestamp_micros(),
        later.timestamp_micros(),
        "a later stored cutoff is preserved"
    );
    assert_eq!(first.password_changed_at, first.cutoff);
    assert!(first.authentication_revoked_at >= first.cutoff);

    let second = cut_off(&db.cutoff, behind.id).await;
    assert_eq!(second.authentication_revoked_at, second.cutoff);
    assert_eq!(second.password_changed_at, second.cutoff);

    let third = cut_off(&db.cutoff, behind.id).await;
    assert!(third.cutoff > second.cutoff);
    assert_eq!(third.authentication_revoked_at, third.cutoff);
    assert_eq!(third.password_changed_at, third.cutoff);

    let stored: (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT authentication_revoked_at, password_changed_at FROM users WHERE id = $1",
    )
    .bind(behind.id)
    .fetch_one(&db.owner)
    .await
    .unwrap();
    assert_eq!(stored, (third.cutoff, third.cutoff));
}

// ---------------------------------------------------------------------------
// Credentials.
// ---------------------------------------------------------------------------

async fn credential_snapshot(pool: &PgPool, user: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object( \
           'user', (SELECT to_jsonb(u) FROM users u WHERE id = $1), \
           'sessions', (SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY id), '[]') \
                        FROM refresh_sessions s WHERE user_id = $1), \
           'resets', (SELECT coalesce(jsonb_agg(to_jsonb(r)), '[]') \
                      FROM password_reset_tokens r WHERE user_id = $1))",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Every credential a user can hold, including a session family from before
/// device binding and one already revoked family.
async fn seed_credentials(db: &Database, user: Uuid, invitation_secret: &str) {
    sqlx::query(
        "UPDATE users SET password_reset_token_hash = $2, \
         password_reset_expires = now() + interval '1 hour', \
         email_verification_token_hash = $3, \
         email_verification_token_expires_at = now() + interval '1 hour', \
         pending_approval_email_transition_id = $4, \
         pending_approval_email_payload = '{\"synthetic\": true}', \
         pending_approval_email_retry_at = now() \
         WHERE id = $1",
    )
    .bind(user)
    .bind(token_digest(invitation_secret))
    .bind(token_digest(&generate_token()))
    .bind(Uuid::new_v4())
    .execute(&db.owner)
    .await
    .unwrap();
    for _ in 0..2 {
        seed_family(db, user).await;
    }
    sqlx::query(
        "INSERT INTO refresh_sessions (id, user_id, family_id, jti_hash, device_id_hash, expires_at) \
         VALUES ($1, $2, $3, $4, NULL, now() + interval '7 days')",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(Uuid::new_v4())
    .bind(token_digest(&generate_token()))
    .execute(&db.owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO refresh_sessions (id, user_id, family_id, jti_hash, device_id_hash, expires_at, revoked) \
         VALUES ($1, $2, $3, $4, $5, now() + interval '7 days', true)",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(Uuid::new_v4())
    .bind(token_digest(&generate_token()))
    .bind(token_digest(&generate_token()))
    .execute(&db.owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at) \
         VALUES ($1, $2, now() + interval '1 hour')",
    )
    .bind(user)
    .bind(token_digest(&format!("reset-{invitation_secret}")))
    .execute(&db.owner)
    .await
    .unwrap();
}

struct PythonService {
    child: Child,
    base: String,
    token: String,
}

impl PythonService {
    async fn start(db: &Database) -> Self {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let token = generate_token();
        let child = python_command("serve", &db.runtime_url)
            .env("OFFBOARDING_HARNESS_PORT", port.to_string())
            .env("OFFBOARDING_HARNESS_TOKEN", &token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("Python interpreter unavailable: set RUST_TEST_PYTHON or put python3 on PATH");
        let mut service = Self {
            child,
            base: format!("http://127.0.0.1:{port}"),
            token,
        };
        let client = client();
        let started = Instant::now();
        loop {
            if let Some(status) = service.child.try_wait().unwrap() {
                panic!("the Python service exited early: {status}");
            }
            let ready = client
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
            .header("x-harness-token", &self.token)
            .json(&json!({"channel": channel, "event": event, "data": {"marker": event}}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
}

impl Drop for PythonService {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn all_refresh_families_and_credentials_are_invalidated() {
    let db = Database::connect().await;
    let subject = db.user(None).await;
    let other = db.user(None).await;
    let subject_secret = generate_token();
    let other_secret = generate_token();
    seed_credentials(&db, subject.id, &subject_secret).await;
    seed_credentials(&db, other.id, &other_secret).await;
    let before = credential_snapshot(&db.owner, subject.id).await;
    let other_before = credential_snapshot(&db.owner, other.id).await;

    let revocation = cut_off(&db.cutoff, subject.id).await;
    assert_eq!(
        revocation.refresh_sessions_revoked, 3,
        "both device families and the family from before device binding"
    );
    assert_eq!(revocation.reset_tokens_removed, 1);

    let after = credential_snapshot(&db.owner, subject.id).await;
    let user = &after["user"];
    for column in [
        "password_reset_token_hash",
        "password_reset_expires",
        "email_verification_token_hash",
        "email_verification_token_expires_at",
        "pending_approval_email_transition_id",
        "pending_approval_email_payload",
        "pending_approval_email_retry_at",
    ] {
        assert!(user[column].is_null(), "{column} must be cleared");
    }
    assert_ne!(user["password_hash"], before["user"]["password_hash"]);
    assert!(!password::verify(PASSWORD, user["password_hash"].as_str().unwrap()).await);
    for column in [
        "email",
        "email_verified",
        "status",
        "organization_id",
        "kyc_status",
        "kyc_review_note",
        "kyc_organization_id",
        "kyc_reviewed_at",
    ] {
        assert_eq!(user[column], before["user"][column], "{column} is kept");
    }
    let sessions = after["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 4, "session rows are kept as history");
    assert!(sessions.iter().all(|session| session["revoked"] == true));
    assert_eq!(after["resets"], json!([]));
    assert_eq!(
        credential_snapshot(&db.owner, other.id).await,
        other_before,
        "another user's credentials are unchanged"
    );

    let mut connection = db.session.acquire().await.unwrap();
    let reset_digest = token_digest(&format!("reset-{subject_secret}"));
    assert_eq!(
        store::reset_token_owner(&mut connection, &reset_digest)
            .await
            .unwrap(),
        None
    );
    drop(connection);

    // Python's reset route resolves the invitation and reset secret it reads
    // from the users row; the subject's no longer resolves, a control's does.
    let service = PythonService::start(&db).await;
    let reset = |secret: String| {
        let url = format!("{}/api/auth/reset-password", service.base);
        async move {
            client()
                .post(url)
                .json(&json!({"token": secret, "new_password": "Replacement-password-123"}))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(reset(subject_secret).await, 400);
    assert_eq!(
        reset(other_secret).await,
        200,
        "the control secret still works"
    );
}

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn old_equal_malformed_and_future_iat_fail_closed() {
    let db = Database::connect().await;
    let organization = db.organization().await;
    let user = db.user(Some(organization)).await;
    let revocation = cut_off(&db.cutoff, user.id).await;
    let cutoff = revocation.cutoff;
    let micros = |value: DateTime<Utc>| json!(value.timestamp_micros());
    let old = cutoff - TimeDelta::minutes(10);
    let ahead = cutoff - TimeDelta::microseconds(1);
    let cases: Vec<(&str, String)> = vec![
        (
            "old",
            access_token(user.id, Some(micros(old)), Some(json!(old.timestamp()))),
        ),
        ("equal", access_token(user.id, Some(micros(cutoff)), None)),
        // Issued on a host whose clock runs ahead of the database, inside the margin.
        (
            "ahead_within_margin",
            access_token(user.id, Some(micros(ahead)), None),
        ),
        (
            "ahead_with_iat",
            access_token(user.id, Some(micros(ahead)), Some(json!(ahead.timestamp()))),
        ),
        ("missing", access_token(user.id, None, None)),
        (
            "text",
            access_token(user.id, Some(json!("1700000000000000")), None),
        ),
        ("fractional", access_token(user.id, Some(json!(1.5)), None)),
        ("negative", access_token(user.id, Some(json!(-1)), None)),
        (
            "out_of_range",
            access_token(user.id, Some(json!(1_000_000_000_000_000_000i64)), None),
        ),
        // Control: issued after the cutoff, so only state can refuse it.
        (
            "issued_after",
            access_token(
                user.id,
                Some(micros(cutoff + TimeDelta::microseconds(1))),
                None,
            ),
        ),
    ];
    let named: Vec<(String, String)> = cases
        .iter()
        .map(|(name, token)| ((*name).to_owned(), token.clone()))
        .collect();

    // The cutoff alone, on an account that is still approved and attached.
    let approved = python_auth(&db, &named).await;
    for (name, token) in &cases {
        let rust = rust_normal(&db.owner, token).await;
        if *name == "issued_after" {
            assert_eq!(rust, Ok(()), "the control token passes Rust");
            assert_eq!(
                approved[name]["normal"], "accepted",
                "the control token passes Python"
            );
        } else {
            assert!(rust.is_err(), "Rust normal route accepted {name}");
            assert!(
                python_refused(&approved[name]["normal"]),
                "Python normal route accepted {name}: {}",
                approved[name]["normal"]
            );
        }
    }
    assert_eq!(
        rust_normal(&db.owner, &cases[1].1).await,
        Err(AuthError::PasswordChanged)
    );
    assert_eq!(
        approved["equal"]["normal"],
        "refused:401:AUTH_PASSWORD_CHANGED"
    );

    // The offboarded account: rejected and detached. The owner-cleanup routes
    // skip the revocation cutoff for a rejected account; the password cutoff
    // still refuses every token issued up to the cutoff.
    db.detach(user.id).await;
    let rejected = python_auth(&db, &named).await;
    for (name, token) in &cases {
        assert!(
            rust_normal(&db.owner, token).await.is_err(),
            "Rust normal route accepted {name}"
        );
        assert!(
            python_refused(&rejected[name]["normal"]),
            "Python normal route accepted {name}"
        );
        let rust = rust_cleanup(&db.owner, token).await;
        if *name == "issued_after" {
            assert_eq!(
                rust,
                Ok(()),
                "the cleanup control shows the check is not vacuous"
            );
            assert_eq!(rejected[name]["cleanup"], "accepted");
        } else {
            assert!(rust.is_err(), "Rust cleanup rule accepted {name}");
            assert!(
                python_refused(&rejected[name]["cleanup"]),
                "Python cleanup route accepted {name}: {}",
                rejected[name]["cleanup"]
            );
        }
    }
    assert_eq!(
        rust_cleanup(&db.owner, &cases[1].1).await,
        Err(AuthError::PasswordChanged)
    );
    assert_eq!(
        rejected["equal"]["cleanup"],
        "refused:401:AUTH_PASSWORD_CHANGED"
    );
    assert_eq!(
        rejected["old"]["cleanup"],
        "refused:401:AUTH_PASSWORD_CHANGED"
    );
}

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn credential_cutoff_requires_the_held_user_lock() {
    let db = Database::connect().await;
    let user = db.user(None).await;
    seed_family(&db, user.id).await;
    let before = credential_snapshot(&db.owner, user.id).await;

    // Another transaction holds the row: refuse at once, write nothing.
    let mut holder = db.owner.begin().await.unwrap();
    lock_user_row(&mut holder, user.id).await;
    let mut transaction = db.cutoff.begin().await.unwrap();
    let started = Instant::now();
    let error = revoke_account_credentials(&mut transaction, user.id)
        .await
        .expect_err("another transaction holds the user row");
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "the helper must not wait"
    );
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    );
    transaction.rollback().await.unwrap();
    holder.rollback().await.unwrap();

    // No internal commit: the caller's rollback discards every change.
    let mut transaction = db.cutoff.begin().await.unwrap();
    lock_user_row(&mut transaction, user.id).await;
    revoke_account_credentials(&mut transaction, user.id)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(credential_snapshot(&db.owner, user.id).await, before);

    let mut transaction = db.cutoff.begin().await.unwrap();
    let missing = revoke_account_credentials(&mut transaction, Uuid::new_v4()).await;
    assert!(matches!(missing, Err(sqlx::Error::RowNotFound)));
    transaction.rollback().await.unwrap();
}

// ---------------------------------------------------------------------------
// Streams and delivery (the unchanged Python code).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Observed {
    frames: Vec<(Instant, String)>,
    closed_at: Option<Instant>,
}

impl Observed {
    fn has(&self, needle: &str) -> bool {
        self.frames.iter().any(|(_, frame)| frame.contains(needle))
    }
}

struct EventStream {
    observed: Arc<Mutex<Observed>>,
    reader: tokio::task::JoinHandle<()>,
}

impl EventStream {
    async fn open(url: String) -> Self {
        let response = client().get(url).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "the stream must open");
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
                            shared.lock().unwrap().frames.push((Instant::now(), frame));
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
        self.observed.lock().unwrap().has(needle)
    }

    fn closed_at(&self) -> Option<Instant> {
        self.observed.lock().unwrap().closed_at
    }

    fn last_frame(&self) -> String {
        let observed = self.observed.lock().unwrap();
        observed
            .frames
            .last()
            .map(|(_, frame)| frame.clone())
            .unwrap_or_default()
    }

    fn received_before_close(&self, needle: &str) -> bool {
        let observed = self.observed.lock().unwrap();
        let revoked = observed
            .frames
            .iter()
            .position(|(_, frame)| frame.contains("auth_revoked"));
        observed
            .frames
            .iter()
            .position(|(_, frame)| frame.contains(needle))
            .is_some_and(|index| revoked.is_none_or(|revoked| index < revoked))
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

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn detached_streams_close_after_membership_recheck() {
    let db = Database::connect().await;
    let organization = db.organization().await;
    let departed = db.user(Some(organization)).await;
    let cut_only = db.user(Some(organization)).await;
    let colleague = db.user(Some(organization)).await;
    let mut replay_after = i64::MAX;
    for _ in 0..2 {
        let marker = format!("replay-{}", Uuid::new_v4());
        let sequence: i64 =
            sqlx::query_scalar(include_str!("fixtures/offboarding_stream_outbox.sql"))
                .bind(Uuid::new_v4())
                .bind(&marker)
                .bind(organization.to_string())
                .fetch_one(&db.owner)
                .await
                .unwrap();
        replay_after = replay_after.min(sequence - 1);
    }
    let service = PythonService::start(&db).await;
    // Each member's stream token is issued once, before the account change; a
    // departed account cannot obtain a new one.
    let tokens: std::collections::HashMap<Uuid, String> = [departed.id, cut_only.id, colleague.id]
        .into_iter()
        .map(|user| {
            (
                user,
                issuer().stream(user, organization, "test", Utc::now()),
            )
        })
        .collect();
    let activity = |user: Uuid| {
        format!(
            "{}/api/stream/activity?stream_token={}",
            service.base, tokens[&user]
        )
    };
    let trades = |user: Uuid| {
        format!(
            "{}/api/stream/trades?stream_token={}",
            service.base, tokens[&user]
        )
    };

    let departed_activity = EventStream::open(activity(departed.id)).await;
    let departed_trades = EventStream::open(format!(
        "{}&last_event_id={replay_after}",
        trades(departed.id)
    ))
    .await;
    let cut_activity = EventStream::open(activity(cut_only.id)).await;
    let cut_trades = EventStream::open(trades(cut_only.id)).await;
    let colleague_activity = EventStream::open(activity(colleague.id)).await;
    let activity_streams = [&departed_activity, &cut_activity, &colleague_activity];
    let trade_streams = [&departed_trades, &cut_trades];

    // Every stream is subscribed before the account changes.
    assert!(
        wait_until(Duration::from_secs(10), || trade_streams
            .iter()
            .all(|stream| stream.has("stream-origin")))
        .await,
        "both trade streams started"
    );
    let mut subscribed = false;
    for _ in 0..10 {
        service
            .publish(&format!("activity:{organization}"), "synthetic_before")
            .await;
        if wait_until(Duration::from_secs(1), || {
            activity_streams
                .iter()
                .all(|stream| stream.has("synthetic_before"))
        })
        .await
        {
            subscribed = true;
            break;
        }
    }
    assert!(subscribed, "every activity stream receives live events");
    assert!(
        departed_trades.has("synthetic_replay"),
        "the reconnecting stream replayed the outbox"
    );

    // One transaction: detach the departed member and cut off both accounts.
    let mut transaction = db.cutoff.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(vec![departed.id, cut_only.id])
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET status = 'REJECTED', organization_id = NULL WHERE id = $1")
        .bind(departed.id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    revoke_account_credentials(&mut transaction, departed.id)
        .await
        .unwrap();
    revoke_account_credentials(&mut transaction, cut_only.id)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let committed = Instant::now();

    // An organization event already in flight after the commit. The streams
    // deliver it until their next recheck: the accepted bounded window.
    service
        .publish(&format!("activity:{organization}"), "synthetic_in_flight")
        .await;
    service
        .publish(&format!("trades:{organization}"), "synthetic_in_flight")
        .await;

    let closing = [
        &departed_activity,
        &departed_trades,
        &cut_activity,
        &cut_trades,
    ];
    assert!(
        wait_until(STREAM_RECHECK + STREAM_SLACK, || closing
            .iter()
            .all(|stream| stream.closed_at().is_some()))
        .await,
        "every affected stream closes within one recheck interval"
    );
    for stream in closing {
        let closed = stream.closed_at().unwrap();
        let window = closed.duration_since(committed);
        assert!(
            window <= STREAM_RECHECK + STREAM_SLACK,
            "closed {window:?} after commit"
        );
        assert!(
            stream.last_frame().contains("event: auth_revoked"),
            "closed by the recheck"
        );
        println!(
            "stream closed {window:?} after commit; in-flight event delivered: {}",
            stream.received_before_close("synthetic_in_flight")
        );
        assert!(
            stream.received_before_close("synthetic_in_flight"),
            "an event in flight before the recheck may still be delivered"
        );
    }
    assert!(
        colleague_activity.closed_at().is_none(),
        "a current member keeps the stream"
    );
    assert!(colleague_activity.has("synthetic_in_flight"));

    // After the recheck nothing more reaches the departed member.
    service
        .publish(&format!("activity:{organization}"), "synthetic_after")
        .await;
    assert!(
        wait_until(Duration::from_secs(5), || colleague_activity
            .has("synthetic_after"))
        .await
    );
    assert!(!departed_activity.has("synthetic_after"));

    // Reconnecting, including outbox replay, is refused.
    for url in [
        format!("{}&last_event_id={replay_after}", trades(departed.id)),
        activity(departed.id),
        trades(cut_only.id),
        activity(cut_only.id),
    ] {
        let status = client().get(url).send().await.unwrap().status().as_u16();
        assert_eq!(status, 401, "a new private stream is refused");
    }
}

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn detachment_suppresses_future_org_notices() {
    let db = Database::connect().await;
    let organization = db.organization().await;
    let departed = db.user(Some(organization)).await;
    let colleague = db.user(Some(organization)).await;
    let request = json!({"organization_id": organization.to_string()});
    let mut both = vec![departed.id.to_string(), colleague.id.to_string()];
    both.sort();
    let before = python("org-notice", &db.owner_url, request.clone()).await;
    assert_eq!(
        before["recipients"],
        json!(both),
        "both members receive a notice first"
    );

    db.detach(departed.id).await;
    cut_off(&db.cutoff, departed.id).await;
    let after = python("org-notice", &db.owner_url, request).await;
    assert_eq!(after["recipients"], json!([colleague.id.to_string()]));
}

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn withdrawn_order_has_no_future_expiry_notice() {
    let db = Database::connect().await;
    let organization = db.organization().await;
    let departed = db.user(Some(organization)).await;
    let colleague = db.user(Some(organization)).await;
    db.detach(departed.id).await;
    cut_off(&db.cutoff, departed.id).await;
    let result = python(
        "expiry-reminder",
        &db.owner_url,
        json!({
            "organization_id": organization.to_string(),
            "withdrawn_owner_id": departed.id.to_string(),
            "recipients": [departed.id.to_string(), colleague.id.to_string()],
        }),
    )
    .await;
    assert_eq!(
        result["reminders"],
        json!([["open", colleague.id.to_string()]]),
        "only the open order reminds, and only a current member"
    );
}

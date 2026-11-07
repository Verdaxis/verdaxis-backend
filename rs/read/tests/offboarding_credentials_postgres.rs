//! PostgreSQL and cross-stack checks of the account credential cutoff.
//!
//! These cases need a migrated disposable database and the project's Python
//!
//! - `OFFBOARDING_TEST_DATABASE_URL`: the runtime application role;
//! - `OFFBOARDING_TEST_OWNER_URL`: the schema owner, used only for fixtures.
//!
#![cfg(feature = "contract-tests")]

use std::sync::Arc;
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
    offboarding::{
        CUTOFF_SKEW_MARGIN_SECONDS, CredentialRevocation, USER_LOCK_NOT_HELD, UnusablePasswordHash,
        revoke_account_credentials,
    },
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
    /// Prepared once, before any lock, as a real caller does.
    unusable: UnusablePasswordHash,
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
            unusable: UnusablePasswordHash::prepare().await.unwrap(),
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

/// Lock the user row and write it, as the account change before a cutoff
/// does: the helper accepts only a row this transaction wrote.
async fn lock_user_row(connection: &mut PgConnection, user: Uuid) {
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET status = status WHERE id = $1")
        .bind(user)
        .execute(&mut *connection)
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
    let unusable = UnusablePasswordHash::prepare().await.unwrap();
    let mut transaction = pool.begin().await.unwrap();
    lock_user_row(&mut transaction, user).await;
    let revocation = revoke_account_credentials(&mut transaction, user, &unusable)
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
    let unusable = UnusablePasswordHash::prepare().await.unwrap();
    let mut transaction = pool.begin().await.unwrap();
    let requested = Instant::now();
    lock_user_row(&mut transaction, user).await;
    println!(
        "the cutoff transaction waited {:?} for the user row",
        requested.elapsed()
    );
    let acquired = database_now(&mut transaction).await;
    let revocation = revoke_account_credentials(&mut transaction, user, &unusable)
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

    // The cutoff transaction holds the lock; a login waits and then fails.
    let user = db.user(None).await;
    let mut transaction = db.cutoff.begin().await.unwrap();
    lock_user_row(&mut transaction, user.id).await;
    revoke_account_credentials(&mut transaction, user.id, &db.unusable)
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
    revoke_account_credentials(&mut transaction, user.id, &db.unusable)
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
        revocation.refresh_families_revoked, 3,
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
    // The cutoff alone, on an account that is still approved and attached.
    for (name, token) in &cases {
        let rust = rust_normal(&db.owner, token).await;
        if *name == "issued_after" {
            assert_eq!(rust, Ok(()), "the control token passes Rust");
        } else {
            assert!(rust.is_err(), "Rust normal route accepted {name}");
        }
    }
    assert_eq!(
        rust_normal(&db.owner, &cases[1].1).await,
        Err(AuthError::PasswordChanged)
    );

    // The offboarded account: rejected and detached. The owner-cleanup routes
    // skip the revocation cutoff for a rejected account; the password cutoff
    // still refuses every token issued up to the cutoff.
    db.detach(user.id).await;
    for (name, token) in &cases {
        assert!(
            rust_normal(&db.owner, token).await.is_err(),
            "Rust normal route accepted {name}"
        );
        let rust = rust_cleanup(&db.owner, token).await;
        if *name == "issued_after" {
            assert_eq!(
                rust,
                Ok(()),
                "the cleanup control shows the check is not vacuous"
            );
        } else {
            assert!(rust.is_err(), "Rust cleanup rule accepted {name}");
        }
    }
    assert_eq!(
        rust_cleanup(&db.owner, &cases[1].1).await,
        Err(AuthError::PasswordChanged)
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
    let error = revoke_account_credentials(&mut transaction, user.id, &db.unusable)
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
    revoke_account_credentials(&mut transaction, user.id, &db.unusable)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(credential_snapshot(&db.owner, user.id).await, before);

    let mut transaction = db.cutoff.begin().await.unwrap();
    let missing = revoke_account_credentials(&mut transaction, Uuid::new_v4(), &db.unusable).await;
    assert!(matches!(missing, Err(sqlx::Error::RowNotFound)));
    transaction.rollback().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a migrated disposable PostgreSQL database and Python"]
async fn credential_cutoff_refuses_without_the_caller_lock() {
    let db = Database::connect().await;
    let user = db.user(None).await;
    seed_family(&db, user.id).await;
    let before = credential_snapshot(&db.owner, user.id).await;
    let refused = |error: &sqlx::Error| matches!(error, sqlx::Error::InvalidArgument(message) if message == USER_LOCK_NOT_HELD);

    // No lock, a foreign-key strength lock, and a lock without a write: each
    // is refused and writes nothing.
    for lock in [
        None,
        Some("SELECT id FROM users WHERE id = $1 FOR KEY SHARE"),
        Some("SELECT id FROM users WHERE id = $1 FOR UPDATE"),
    ] {
        let mut transaction = db.cutoff.begin().await.unwrap();
        if let Some(statement) = lock {
            sqlx::query(statement)
                .bind(user.id)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        }
        let error = revoke_account_credentials(&mut transaction, user.id, &db.unusable)
            .await
            .expect_err("the caller has not written the user row");
        assert!(refused(&error), "{lock:?}: {error:?}");
        transaction.rollback().await.unwrap();
        assert_eq!(credential_snapshot(&db.owner, user.id).await, before);
    }

    // A connection outside any transaction holds nothing.
    let mut connection = db.cutoff.acquire().await.unwrap();
    let error = revoke_account_credentials(&mut connection, user.id, &db.unusable)
        .await
        .expect_err("an autocommit connection holds no row lock");
    assert!(refused(&error), "{error:?}");
    drop(connection);
    assert_eq!(credential_snapshot(&db.owner, user.id).await, before);

    // A row this transaction wrote is held until it ends: accepted.
    let mut transaction = db.cutoff.begin().await.unwrap();
    sqlx::query("UPDATE users SET status = 'REJECTED' WHERE id = $1")
        .bind(user.id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    revoke_account_credentials(&mut transaction, user.id, &db.unusable)
        .await
        .expect("a written row is held by this transaction");
    transaction.rollback().await.unwrap();
    assert_eq!(credential_snapshot(&db.owner, user.id).await, before);
}

// ---------------------------------------------------------------------------

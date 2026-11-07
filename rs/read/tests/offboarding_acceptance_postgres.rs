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
//! a loopback server for its private streams through
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
    net::SocketAddr,
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
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;
use verdaxis_read::{
    AppState, Config,
    auth::{TokenIssuer, VerificationSettings},
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
    pool: PgPool,
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
        let pool = runtime_pool(&database.runtime_url).await;
        let state = application(&database.runtime_url, pool.clone());
        let served = verdaxis_read::router(state.clone());
        Self {
            database,
            admin,
            pool,
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

    async fn request_rows(&self, requests: Vec<Value>) -> Value {
        let mut results = serde_json::Map::new();
        for request in requests {
            let mut builder = Request::builder()
                .method(request["method"].as_str().unwrap())
                .uri(request["path"].as_str().unwrap())
                .header("host", "localhost");
            if let Some(token) = request["token"].as_str() {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            if let Some(headers) = request["headers"].as_object() {
                for (name, value) in headers {
                    builder = builder.header(name, value.as_str().unwrap());
                }
            }
            if let Some(cookies) = request["cookies"].as_object() {
                let value = cookies
                    .iter()
                    .map(|(name, value)| format!("{name}={}", value.as_str().unwrap()))
                    .collect::<Vec<_>>()
                    .join("; ");
                builder = builder.header("cookie", value);
            }
            let body = if let Some(form) = request["form"].as_object() {
                builder = builder.header("content-type", "application/x-www-form-urlencoded");
                let mut encoded = url::form_urlencoded::Serializer::new(String::new());
                for (name, value) in form {
                    encoded.append_pair(name, value.as_str().unwrap());
                }
                Body::from(encoded.finish())
            } else {
                builder = builder.header("content-type", "application/json");
                request
                    .get("json")
                    .map_or_else(Body::empty, |value| Body::from(value.to_string()))
            };
            let response = self
                .served
                .clone()
                .oneshot(builder.body(body).unwrap())
                .await
                .unwrap();
            let status = response.status().as_u16();
            let mut cookies = serde_json::Map::new();
            for header in response.headers().get_all("set-cookie") {
                if let Some((name, value)) = header
                    .to_str()
                    .unwrap()
                    .split(';')
                    .next()
                    .unwrap()
                    .split_once('=')
                {
                    cookies.insert(name.to_owned(), json!(value));
                }
            }
            let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            results.insert(
                request["name"].as_str().unwrap().to_owned(),
                json!({"status":status,"body":body,"cookies":cookies}),
            );
        }
        Value::Object(results)
    }
}

// ---------------------------------------------------------------------------
// The unchanged Python application.
// ---------------------------------------------------------------------------

fn request_status(results: &Value, name: &str) -> u16 {
    results[name]["status"]
        .as_u64()
        .unwrap_or_else(|| panic!("no Python result {name}: {results}")) as u16
}

fn request_cookies(results: &Value, name: &str) -> Value {
    results[name]["cookies"].clone()
}

fn request_token(results: &Value, name: &str) -> String {
    results[name]["body"]["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("no access token in {name}: {results}"))
        .to_owned()
}

fn login_request(name: &str, email: &str, password: &str) -> Value {
    json!({"name": name, "method": "POST", "path": "/api/auth/login",
           "form": {"username": email, "password": password}})
}

fn approve_request(name: &str, user: Uuid) -> Value {
    json!({"name": name, "method": "PUT", "path": format!("/api/auth/approve/{user}"),
           "token": token(ADMIN)})
}

fn join_approval_request(name: &str, join: Uuid) -> Value {
    json!({"name": name, "method": "PUT",
           "path": format!("/api/auth/organization-joins/{join}/approve"),
           "token": token(ADMIN), "json": {"review_note": "Verified company request"}})
}

fn trade_request(name: &str, trade: Uuid, action: &str, bearer: &str) -> Value {
    let (method, body) = if action == "pay" {
        ("POST", Value::Null)
    } else {
        ("PUT", deliver_body())
    };
    json!({"name": name, "method": method, "path": format!("/api/trades/{trade}/{action}"),
           "token": bearer, "json": body})
}

/// The Python application as a loopback server, for its two private streams.
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

/// The backend waiting at the fixture gate.
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
    let rust = format!("http://127.0.0.1:{}", serve(world.served.clone()).await);
    let stream_token = |user| issuer().stream(user, ORGANIZATION, "test", Utc::now());
    let subject_stream = stream_token(SUBJECT);
    let colleague_stream = stream_token(COLLEAGUE);
    let rust_trades = |bearer: &str| {
        format!("{rust}/api/stream/trades?stream_token={bearer}&last_event_id={cursor}")
    };

    let streams = [(
        "Rust trades",
        EventStream::open(rust_trades(&subject_stream)).await,
    )];
    let colleague_rust = EventStream::open(rust_trades(&colleague_stream)).await;
    // Both trade streams replayed the outbox before the change.
    for (name, stream) in &streams {
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
    // Before the change both stacks accept the person's current token.
    let old = token(SUBJECT);
    let (status, body) = send(&world.served, "GET", "/api/auth/me", Some(&old), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

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
    for url in [rust_trades(&subject_stream)] {
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
    for (name, bearer) in &tokens {
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
        .request_rows(vec![
            approve_request("approve", SUBJECT),
            join_approval_request("join", REJOIN_REQUEST),
        ])
        .await;
    assert!(request_status(&results, "approve") >= 400, "{results}");
    assert!(request_status(&results, "join") >= 400, "{results}");
    let after = world.account(SUBJECT).await;
    assert_eq!(after["status"], before["status"]);
    assert_eq!(after["organization_id"], before["organization_id"]);
    assert_eq!(after["offboarded_at"], before["offboarded_at"]);
    assert_eq!(after["joins"][REJOIN_REQUEST.to_string()], "PENDING");
    eprintln!(
        "Python approval {} and join approval {} refused",
        request_status(&results, "approve"),
        request_status(&results, "join")
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
        .request_rows(vec![login_request("login", &email, PASSWORD)])
        .await;
    assert_eq!(request_status(&results, "login"), 200, "{results}");
    let old_access = request_token(&results, "login");
    let old_cookies = request_cookies(&results, "login");
    assert!(old_cookies["refresh_token"].is_string(), "{results}");

    let (status, body) = world.offboard(SUBJECT, Some(ORGANIZATION)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let offboarded = assert_offboarded(&mut world, SUBJECT, "offboarded").await;
    assert_ne!(offboarded["kyc_status"], "APPROVED");
    assert!(offboarded["kyc_organization_id"].is_null());

    // The same address cannot become a second account.
    let results = world
        .request_rows(vec![json!({
            "name": "invite", "method": "POST", "path": "/api/auth/admin/invitations",
            "token": token(ADMIN),
            "json": {"email": email, "first_name": "Returning", "role": "SUPPLIER",
                     "organization_id": ORGANIZATION},
        })])
        .await;
    assert_eq!(request_status(&results, "invite"), 409, "{results}");
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
        let old_access = old_access.clone();
        let served = world.served.clone();
        async move {
            let (status, body) =
                send(&served, "GET", "/api/auth/me", Some(&old_access), None).await;
            assert!(refused(status), "{stage}: Rust old token {status} {body}");
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
        .request_rows(vec![
            approve_request("approve", SUBJECT),
            join_approval_request("join", REJOIN_REQUEST),
        ])
        .await;
    assert_eq!(request_status(&results, "approve"), 200, "{results}");
    assert_eq!(request_status(&results, "join"), 200, "{results}");
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
        .request_rows(vec![
            json!({"name": "reset", "method": "POST", "path": "/api/auth/reset-password",
                   "json": {"token": secret, "new_password": NEW_PASSWORD}}),
            login_request("login", &email, NEW_PASSWORD),
        ])
        .await;
    assert_eq!(request_status(&results, "reset"), 200, "{results}");
    assert_eq!(request_status(&results, "login"), 200, "{results}");
    let fresh = request_token(&results, "login");
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
        .request_rows(vec![json!({
            "name": "withdraw", "method": "DELETE",
            "path": format!("/api/orderbook/{COLLEAGUE_ASK}"), "token": token(COLLEAGUE),
        })])
        .await;
    assert!(
        (200..300).contains(&request_status(&results, "withdraw")),
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
        .request_rows(vec![
            trade_request("deliver", CONFIRMED, "deliver", &old),
            login_request("login", &email, PASSWORD),
        ])
        .await;
    assert!(request_status(&results, "deliver") >= 400, "{results}");
    assert!(
        matches!(request_status(&results, "login"), 401 | 403),
        "{results}"
    );
    let trade = world.trade(CONFIRMED).await;
    assert_eq!(trade["status"], "CONFIRMED");
    assert_eq!(trade["seller_user_id"], json!(SUBJECT));
}

// ---------------------------------------------------------------------------
// 5. Python session writers as lock holders.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 6. Python approval and join approval.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 7. Logout, recovery and invitation.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 8. Python settlement and the responsible-person reassignment.
// ---------------------------------------------------------------------------

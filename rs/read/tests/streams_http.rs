//! The three market stream routes and their HTTP contract.
//!
//! The offline cases need no database. The stream subsystem is absent for the
//! unavailable paths and present with a real hub for the live ones. Their pools
//! name a Unix socket in a directory that does not exist and are never dialled;
//! the case that needs an unreachable account store closes its pool first.
//!
//! The two cases marked `ignore` serve the private route against the
//! disposable database that `tests/postgres/test_phase6_http.py` prepares.

use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, BodyDataStream},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
};
use futures_util::StreamExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    AppState, Config, MarketPolicy,
    auth::VerificationSettings,
    runtime::{db::Pools, http::router_with},
    streams::{
        bus::{PublicChannel, StoredEvent, StreamHub},
        http::{Streams, StreamsState, routes},
        policy::{
            MAX_CHANNEL_SUBSCRIBERS, MAX_PROCESS_SUBSCRIBERS, MAX_QUEUED_EVENTS, StreamPolicy,
        },
        replay::ReplayFrame,
    },
};

const SECRET: &str = "test-secret-key-for-streams-minimum-32";
const ISSUER: &str = "verdaxis-test-api";
const AUDIENCE: &str = "verdaxis-test-web";
/// The browser origin the shared CORS layer trusts in the test environment.
const TRUSTED_ORIGIN: &str = "https://test";
/// Offline pools name a Unix socket in a directory that does not exist, so no
/// case can reach whatever listens on a host port.
const NO_DATABASE: &str =
    "postgres://fixture@localhost/verdaxis_test?host=/nonexistent-verdaxis-stream-tests";
/// The validated configuration must name a loopback test database. Offline
/// cases never dial it: their pools are built from [`NO_DATABASE`].
const OFFLINE_CONFIG_URL: &str = "postgres://fixture:fixture-secret@127.0.0.1:1/verdaxis_test";
/// How long a case waits for a frame that is already on its way.
const FRAME_BOUND: Duration = Duration::from_secs(5);
/// The longest silence on an open stream: the 30 second heartbeat.
const HEARTBEAT_BOUND: Duration = Duration::from_secs(30);
/// Allowance for timer delivery on a loaded test host.
const TIMER_SLACK: Duration = Duration::from_secs(3);

const CANONICAL_TRADES: &str = "/api/stream/trades";
const ALIAS_TRADES: &str = "/api/stream/trades/";

const PUBLIC_ROUTES: [(&str, PublicChannel); 4] = [
    ("/api/stream/prices", PublicChannel::Prices),
    ("/api/stream/prices/", PublicChannel::Prices),
    ("/api/stream/orderbook", PublicChannel::Orderbook),
    ("/api/stream/orderbook/", PublicChannel::Orderbook),
];

const ALL_ROUTES: [&str; 6] = [
    "/api/stream/prices",
    "/api/stream/prices/",
    "/api/stream/orderbook",
    "/api/stream/orderbook/",
    CANONICAL_TRADES,
    ALIAS_TRADES,
];

fn config(database_url: &str) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: database_url.into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().expect("loopback bind"),
        read_admission_slots: 1,
        pool_max: 2,
        python_budget: 0,
        configured_max_connections: 100,
        phase2: Default::default(),
    }
}

/// A one-connection pool that connects only when a request needs it.
fn lazy_pool(database_url: &str) -> PgPool {
    let options: PgConnectOptions = database_url.parse().expect("database URL");
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .connect_lazy_with(options)
}

/// Application state configured with `config_url` whose read pool is `read`.
/// Stream admission and replay draw only from that pool; the health pool is
/// never used here.
fn state_with(config_url: &str, read: PgPool, streams: Option<Streams>) -> AppState {
    let policy = MarketPolicy {
        source_sha: verdaxis_read::compiled_sha().into(),
        products: vec![],
        points: vec![],
    };
    let pools = Pools {
        read,
        health: lazy_pool(NO_DATABASE),
    };
    let mut state = AppState::new(config(config_url), pools, policy).expect("test state");
    state.streams = streams.map(Arc::new);
    state
}

fn offline_state(streams: Option<Streams>) -> AppState {
    state_with(OFFLINE_CONFIG_URL, lazy_pool(NO_DATABASE), streams)
}

fn streams(hub: &StreamHub) -> Streams {
    Streams {
        hub: hub.clone(),
        verification: VerificationSettings::new(
            SECRET.into(),
            None,
            ISSUER.into(),
            AUDIENCE.into(),
        ),
    }
}

/// The stream router alone.
fn isolated(state: AppState) -> Router {
    routes().with_state(StreamsState {
        state: Arc::new(state),
    })
}

/// The stream routes inside the serving composition: the shared fallbacks,
/// CORS and the request log.
fn served(state: AppState) -> Router {
    let stream_routes = routes().with_state(StreamsState {
        state: Arc::new(state.clone()),
    });
    router_with(stream_routes, state)
}

fn live(hub: &StreamHub) -> Router {
    isolated(offline_state(Some(streams(hub))))
}

fn hub_with(per_channel: usize, process: usize) -> StreamHub {
    StreamHub::new(
        StreamPolicy::new(per_channel, process, MAX_QUEUED_EVENTS).expect("bounded stream policy"),
    )
}

/// A signed stream token for `user` in `organization`, valid for one minute.
fn token(user: Uuid, organization: Uuid) -> String {
    let now = chrono::Utc::now();
    let claims = json!({
        "sub": user,
        "org_id": organization,
        "environment": "test",
        "type": "stream",
        "jti": Uuid::new_v4(),
        "iss": ISSUER,
        "aud": AUDIENCE,
        "iat": now.timestamp(),
        "exp": (now + chrono::Duration::seconds(60)).timestamp(),
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("signed stream token")
}

fn get(path: &str) -> Request<Body> {
    request("GET", path, &[])
}

fn request(method: &str, path: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::empty()).expect("request")
}

async fn call(router: &Router, request: Request<Body>) -> Response {
    router
        .clone()
        .oneshot(request)
        .await
        .expect("infallible router")
}

/// A finite response read whole. Live stream bodies use [`Frames`] instead.
async fn send(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = call(router, request).await;
    let status = response.status();
    let headers = response.headers().clone();
    let body = tokio::time::timeout(
        FRAME_BOUND,
        axum::body::to_bytes(response.into_body(), 64 * 1024),
    )
    .await
    .expect("a finite body")
    .expect("body bytes");
    (status, headers, body.to_vec())
}

fn code(bytes: &[u8]) -> String {
    let body: Value = serde_json::from_slice(bytes).expect("a JSON error body");
    body["detail"]["code"]
        .as_str()
        .expect("a code in the error detail")
        .to_owned()
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn is_event_stream(headers: &HeaderMap) -> bool {
    header_text(headers, "content-type").is_some_and(|value| value.starts_with("text/event-stream"))
}

fn assert_stream_headers(headers: &HeaderMap, context: &str) {
    assert!(is_event_stream(headers), "{context}: {headers:?}");
    assert_eq!(
        header_text(headers, "cache-control"),
        Some("no-cache"),
        "{context}"
    );
    assert_eq!(
        header_text(headers, "x-accel-buffering"),
        Some("no"),
        "{context}"
    );
}

/// Whole SSE frames read from a live body, each within a bound, so a body
/// that never ends cannot hang a case and a missing frame fails it.
struct Frames {
    body: BodyDataStream,
    buffer: Vec<u8>,
}

impl Frames {
    fn new(response: Response) -> Self {
        Self {
            body: response.into_body().into_data_stream(),
            buffer: Vec::new(),
        }
    }

    /// Open a stream; it must be admitted with the stream headers.
    async fn open(router: &Router, request: Request<Body>) -> Self {
        let context = format!("{} {}", request.method(), request.uri());
        let response = call(router, request).await;
        assert_eq!(response.status(), StatusCode::OK, "{context}");
        assert_stream_headers(response.headers(), &context);
        Self::new(response)
    }

    /// The next frame without its blank-line terminator, or `None` when the
    /// body ends. A frame that does not arrive within `bound` fails the case.
    async fn next_within(&mut self, bound: Duration) -> Option<String> {
        let read = async {
            loop {
                if let Some(end) = self.buffer.windows(2).position(|pair| pair == b"\n\n") {
                    let frame: Vec<u8> = self.buffer.drain(..end + 2).take(end).collect();
                    return Some(String::from_utf8(frame).expect("a UTF-8 frame"));
                }
                match self.body.next().await {
                    Some(chunk) => self.buffer.extend_from_slice(&chunk.expect("a body chunk")),
                    None => {
                        assert!(self.buffer.is_empty(), "the body ended inside a frame");
                        return None;
                    }
                }
            }
        };
        tokio::time::timeout(bound, read)
            .await
            .expect("no frame within the bound")
    }

    async fn next(&mut self) -> String {
        self.next_within(FRAME_BOUND)
            .await
            .expect("the body ended before the next frame")
    }

    async fn assert_end(&mut self) {
        assert_eq!(
            self.next_within(FRAME_BOUND).await,
            None,
            "the body continued after its terminal frame"
        );
    }
}

/// The frame's lines as `(field, value)`; a comment line has the field "".
fn fields(frame: &str) -> Vec<(String, String)> {
    frame
        .split('\n')
        .map(|line| match line.split_once(':') {
            Some((field, value)) => (
                field.to_owned(),
                value.strip_prefix(' ').unwrap_or(value).to_owned(),
            ),
            None => (line.to_owned(), String::new()),
        })
        .collect()
}

/// The one value of `name` in a frame.
fn single(frame: &str, name: &str) -> String {
    let values: Vec<String> = fields(frame)
        .into_iter()
        .filter(|(field, _)| field == name)
        .map(|(_, value)| value)
        .collect();
    assert_eq!(values.len(), 1, "one `{name}` line in {frame:?}");
    values.into_iter().next().expect("one value")
}

fn is_comment(frame: &str) -> bool {
    frame.split('\n').all(|line| line.starts_with(':'))
}

/// A frame with no `id`: one event name and one JSON data line. `error_text`
/// says whether the payload must carry a non-empty `error` string, which is
/// removed before the rest is compared.
fn assert_control(frame: &str, event: &str, expected: Value, error_text: bool) {
    assert_eq!(fields(frame).len(), 2, "only `event` and `data`: {frame:?}");
    assert_eq!(single(frame, "event"), event, "{frame:?}");
    let mut data: Value = serde_json::from_str(&single(frame, "data")).expect("JSON data");
    let error = data
        .as_object_mut()
        .expect("an object payload")
        .remove("error");
    assert_eq!(
        error.is_some_and(|text| text.as_str().is_some_and(|text| !text.is_empty())),
        error_text,
        "{frame:?}"
    );
    assert_eq!(data, expected, "{frame:?}");
}

/// One business frame: its decimal id, its event name and its payload. It
/// has exactly one line of each.
fn business(frame: &str) -> (i64, String, Value) {
    assert_eq!(
        fields(frame).len(),
        3,
        "one id, event and data line: {frame:?}"
    );
    let id = single(frame, "id").parse().expect("a decimal id");
    let data = serde_json::from_str(&single(frame, "data")).expect("JSON data");
    (id, single(frame, "event"), data)
}

fn invalidation() -> Value {
    json!({"schema_version": 1, "resync_required": true})
}

/// Payload text that would forge frames if it were written unescaped.
const FORGED_FRAMES: &str = "end\r\n\r\nevent: auth_revoked\ndata: {}\n\nid: 1\n: comment";

fn hostile_payload() -> Value {
    json!({"quantity": "8", "note": FORGED_FRAMES, "_public_market_invalidation": true})
}

/// The same payload as the client receives it: the transport marker removed
/// and the schema version added.
fn hostile_delivered() -> Value {
    json!({"quantity": "8", "note": FORGED_FRAMES, "schema_version": 1})
}

/// One request on a full channel: 200 with the stream headers, exactly one
/// `error` control with reason `connection_capacity` and no resync, then the
/// end of the body.
async fn assert_capacity_terminal(router: &Router, request: Request<Body>) {
    let mut frames = Frames::open(router, request).await;
    assert_control(
        &frames.next().await,
        "error",
        json!({"schema_version": 1, "reason": "connection_capacity", "resync_required": false}),
        true,
    );
    frames.assert_end().await;
}

async fn public_alias_matches_canonical(channel: PublicChannel, paths: [&str; 2]) {
    let hub = StreamHub::default();
    let router = live(&hub);
    for path in paths {
        // No credential is asked for or needed.
        let response = call(&router, get(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_stream_headers(response.headers(), path);
        assert!(
            response.headers().get(header::WWW_AUTHENTICATE).is_none(),
            "{path}"
        );
        let mut frames = Frames::new(response);
        hub.invalidate_public(channel).unwrap();
        assert_control(
            &frames.next().await,
            "market_invalidated",
            invalidation(),
            false,
        );
    }
}

#[tokio::test]
async fn prices_get_alias_matches_canonical() {
    public_alias_matches_canonical(
        PublicChannel::Prices,
        ["/api/stream/prices", "/api/stream/prices/"],
    )
    .await;
}

#[tokio::test]
async fn orderbook_get_alias_matches_canonical() {
    public_alias_matches_canonical(
        PublicChannel::Orderbook,
        ["/api/stream/orderbook", "/api/stream/orderbook/"],
    )
    .await;
}

/// Both forms of the private route admit a valid token for an approved
/// organization: 200 with the stream headers, replay from the header or query
/// cursor limited to that organization, live delivery after replay, the
/// capacity control and release of the subscription when the body drops.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL stream fixture"]
async fn trades_get_alias_matches_canonical() {
    let fixture = postgres::Fixture::connect().await;
    let organization = fixture.organization;
    let other = fixture.other_organization;
    let base = fixture.baseline().await;
    fixture
        .insert(base + 1, &[organization], json!({"quantity": "5"}))
        .await;
    fixture
        .insert(base + 2, &[other], json!({"quantity": "6"}))
        .await;
    fixture
        .insert(base + 3, &[other, organization], json!({"quantity": "7"}))
        .await;
    fixture
        .insert(base + 4, &[organization], hostile_payload())
        .await;
    // One subscriber per channel: a second stream for the organization meets
    // the capacity control while the first one lives.
    let hub = hub_with(1, MAX_PROCESS_SUBSCRIBERS);
    let router = isolated(state_with(
        &fixture.app_url,
        lazy_pool(&fixture.app_url),
        Some(streams(&hub)),
    ));
    let trades = |path: &str, query: &str| {
        format!(
            "{path}?stream_token={}{query}",
            token(fixture.user, organization)
        )
    };

    // Canonical path with a header cursor: replay holds only this
    // organization's rows, and the first frame is the first event.
    let cursor = base.to_string();
    let mut stream = Frames::open(
        &router,
        request(
            "GET",
            &trades(CANONICAL_TRADES, ""),
            &[("last-event-id", cursor.as_str())],
        ),
    )
    .await;
    let first = stream.next().await;
    assert!(!first.starts_with(':'), "a comment came first: {first:?}");
    assert_eq!(
        business(&first),
        (
            base + 1,
            "trade_created".to_owned(),
            json!({"quantity": "5", "schema_version": 1})
        )
    );
    assert_eq!(business(&stream.next().await).0, base + 3);
    assert_eq!(
        business(&stream.next().await),
        (base + 4, "trade_created".to_owned(), hostile_delivered())
    );
    println!("stream http postgres: header cursor replay is scoped to the organization");

    // Live delivery follows replay; another organization's event never
    // reaches this body.
    hub.publish_private(live_event(base + 5, &[other])).unwrap();
    hub.publish_private(live_event(base + 6, &[organization]))
        .unwrap();
    assert_eq!(business(&stream.next().await).0, base + 6);
    println!("stream http postgres: live delivery follows replay without foreign events");

    // The organization's channel is full while this body lives.
    assert_capacity_terminal(&router, get(&trades(CANONICAL_TRADES, ""))).await;
    drop(stream);
    assert_eq!(hub.subscriber_count(), 0, "the dropped body kept its slot");
    println!(
        "stream http postgres: capacity is one in-band control; a dropped body frees its slot"
    );

    // The alias path: the header cursor wins over the query cursor, and an
    // idle stream sends a comment within the heartbeat bound.
    let header_cursor = (base + 3).to_string();
    let mut stream = Frames::open(
        &router,
        request(
            "GET",
            &trades(ALIAS_TRADES, &format!("&last_event_id={base}")),
            &[("last-event-id", header_cursor.as_str())],
        ),
    )
    .await;
    assert_eq!(business(&stream.next().await).0, base + 4);
    let heartbeat = stream
        .next_within(HEARTBEAT_BOUND + TIMER_SLACK)
        .await
        .expect("the stream stays open");
    assert!(is_comment(&heartbeat), "{heartbeat:?}");
    assert!(!heartbeat.to_lowercase().contains("pid"), "{heartbeat:?}");
    drop(stream);
    println!("stream http postgres: alias path, header cursor precedence and heartbeat");

    // The query cursor alone.
    let mut stream = Frames::open(
        &router,
        get(&trades(
            ALIAS_TRADES,
            &format!("&last_event_id={}", base + 3),
        )),
    )
    .await;
    assert_eq!(business(&stream.next().await).0, base + 4);
    drop(stream);
    assert_eq!(hub.subscriber_count(), 0);
    println!("stream http postgres: query cursor replay");
}

/// Current account and organization state decides admission before any
/// stream header: a refusal is 401 `STREAM_TOKEN_INVALID`, and an account
/// store that does not answer within its bounds is 503.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL stream fixture"]
async fn trades_database_admission_refusals() {
    let fixture = postgres::Fixture::connect().await;
    let hub = StreamHub::default();
    let router = isolated(state_with(
        &fixture.app_url,
        lazy_pool(&fixture.app_url),
        Some(streams(&hub)),
    ));
    let refused = [
        (
            "organization not approved",
            fixture.pending_user,
            fixture.pending_organization,
        ),
        (
            "token organization is not the account's",
            fixture.user,
            fixture.other_organization,
        ),
        ("unknown account", Uuid::new_v4(), fixture.organization),
    ];
    for (case, user, organization) in refused {
        assert_refused(
            &router,
            user,
            organization,
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_INVALID",
            case,
        )
        .await;
    }
    fixture.set_must_change_password(true).await;
    assert_refused(
        &router,
        fixture.user,
        fixture.organization,
        StatusCode::UNAUTHORIZED,
        "STREAM_TOKEN_INVALID",
        "account must change its password",
    )
    .await;
    fixture.set_must_change_password(false).await;
    println!("stream http postgres: account and organization refusals precede headers");

    // A locked account table outlasts the admission lock bound.
    let mut lock = fixture.owner.begin().await.expect("owner transaction");
    sqlx::query("LOCK TABLE public.users IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .expect("account table lock");
    assert_refused(
        &router,
        fixture.user,
        fixture.organization,
        StatusCode::SERVICE_UNAVAILABLE,
        "STREAM_AUTH_UNAVAILABLE",
        "account store locked",
    )
    .await;
    lock.rollback().await.expect("release the lock");
    // The same account is admitted once the store answers again.
    let stream = Frames::open(
        &router,
        get(&format!(
            "{CANONICAL_TRADES}?stream_token={}",
            token(fixture.user, fixture.organization)
        )),
    )
    .await;
    drop(stream);
    assert_eq!(hub.subscriber_count(), 0);
    println!("stream http postgres: an unanswered account store is 503 before headers");
}

async fn assert_refused(
    router: &Router,
    user: Uuid,
    organization: Uuid,
    status: StatusCode,
    expected: &str,
    case: &str,
) {
    let path = format!(
        "{CANONICAL_TRADES}?stream_token={}",
        token(user, organization)
    );
    let (actual, headers, bytes) = send(router, get(&path)).await;
    assert_eq!(actual, status, "{case}");
    assert_eq!(code(&bytes), expected, "{case}");
    assert!(!is_event_stream(&headers), "{case}: a stream header");
}

fn live_event(seq: i64, participants: &[Uuid]) -> Arc<StoredEvent> {
    Arc::new(
        StoredEvent::new(
            seq,
            participants.to_vec(),
            "trade_created",
            json!({"quantity": "9"}),
        )
        .expect("a valid live event"),
    )
}

#[tokio::test]
async fn trades_rejects_bearer_400_stream_auth_conflict() {
    let bearer = request(
        "GET",
        "/api/stream/trades?stream_token=x",
        &[("authorization", "bearer something")],
    );
    let (status, _, bytes) = send(&live(&StreamHub::default()), bearer).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_AUTH_CONFLICT");
}

#[tokio::test]
async fn trades_rejects_bad_cursor_400_stream_cursor_invalid() {
    let router = live(&StreamHub::default());
    let (status, _, bytes) = send(
        &router,
        get("/api/stream/trades?stream_token=x&last_event_id=abc"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
    // The cursor is checked before the token.
    let (status, _, bytes) = send(&router, get("/api/stream/trades?last_event_id=abc")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
}

#[tokio::test]
async fn trades_requires_token_401_stream_token_required() {
    let router = live(&StreamHub::default());
    for path in [CANONICAL_TRADES, ALIAS_TRADES] {
        let (status, headers, bytes) = send(&router, get(path)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED", "{path}");
        assert!(!is_event_stream(&headers), "{path}");
    }
}

#[tokio::test]
async fn trades_rejects_bad_token_401_stream_token_invalid() {
    let request = get("/api/stream/trades?stream_token=not-a-token");
    let (status, headers, bytes) = send(&live(&StreamHub::default()), request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_INVALID");
    assert!(!is_event_stream(&headers));
}

#[tokio::test]
async fn trades_auth_store_down_503_stream_auth_unavailable() {
    // The account store is unreachable: its pool is closed before the
    // request, so admission fails when it asks for a connection and no
    // address is dialled.
    let read = lazy_pool(NO_DATABASE);
    read.close().await;
    let hub = StreamHub::default();
    let router = isolated(state_with(OFFLINE_CONFIG_URL, read, Some(streams(&hub))));
    for path in [CANONICAL_TRADES, ALIAS_TRADES] {
        // The token must verify for admission to reach the store.
        let path = format!(
            "{path}?stream_token={}",
            token(Uuid::new_v4(), Uuid::new_v4())
        );
        let (status, headers, bytes) = send(&router, get(&path)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(code(&bytes), "STREAM_AUTH_UNAVAILABLE", "{path}");
        assert!(!is_event_stream(&headers), "{path}");
    }
    assert_eq!(hub.subscriber_count(), 0);
}

/// GET is the only method. Every other method, on every canonical and alias
/// path, gets the stream-local refusal with `Allow: GET`, also inside the
/// serving composition, whose shared method fallback must not replace it.
#[tokio::test]
async fn stream_routes_reject_head_and_post_405_envelope() {
    let hub = StreamHub::default();
    let routers = [
        ("isolated", live(&hub)),
        ("served", served(offline_state(Some(streams(&hub))))),
    ];
    for (composition, router) in routers {
        for path in ALL_ROUTES {
            for method in ["HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE"] {
                let context = format!("{composition} {method} {path}");
                let (status, headers, bytes) = send(&router, request(method, path, &[])).await;
                assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{context}");
                let allow: Vec<&str> = headers
                    .get_all(header::ALLOW)
                    .iter()
                    .map(|value| value.to_str().expect("visible ASCII"))
                    .collect();
                assert_eq!(allow, ["GET"], "{context}");
                assert!(
                    header_text(&headers, "content-type")
                        .is_some_and(|value| value.starts_with("application/json")),
                    "{context}: {headers:?}"
                );
                if method == "HEAD" {
                    assert!(bytes.is_empty(), "{context}: a HEAD response has no body");
                } else {
                    assert_eq!(code(&bytes), "STREAM_METHOD_NOT_ALLOWED", "{context}");
                }
            }
        }
    }
    assert_eq!(hub.subscriber_count(), 0);
}

#[tokio::test]
async fn activity_route_is_404_in_rust() {
    let (status, _, _) = send(&live(&StreamHub::default()), get("/api/stream/activity")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn issuer_route_is_not_mounted() {
    let (status, _, _) = send(&live(&StreamHub::default()), get("/api/auth/stream-token")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stream_subsystem_down_503_stream_unavailable() {
    let router = isolated(offline_state(None));
    for path in ALL_ROUTES {
        let (status, headers, bytes) = send(&router, get(path)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(code(&bytes), "STREAM_UNAVAILABLE", "{path}");
        assert!(!is_event_stream(&headers), "{path}");
    }
}

/// Which cursor admission reads is visible through what it refuses next: a
/// valid cursor lets the request reach token validity (401), an invalid one
/// stops it at the cursor (400).
#[tokio::test]
async fn stream_cursor_header_precedes_query() {
    let router = live(&StreamHub::default());
    let cases: [(&str, Option<HeaderValue>, StatusCode, &str); 5] = [
        // A valid header wins over an invalid query value.
        (
            "abc",
            Some(HeaderValue::from_static("5")),
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_INVALID",
        ),
        // An invalid header wins over a valid query value.
        (
            "5",
            Some(HeaderValue::from_static("abc")),
            StatusCode::BAD_REQUEST,
            "STREAM_CURSOR_INVALID",
        ),
        // A header that is not visible ASCII is present, and invalid.
        (
            "5",
            Some(HeaderValue::from_bytes(b"\xff5").expect("opaque header bytes")),
            StatusCode::BAD_REQUEST,
            "STREAM_CURSOR_INVALID",
        ),
        // An empty header falls back to the query value.
        (
            "abc",
            Some(HeaderValue::from_static("")),
            StatusCode::BAD_REQUEST,
            "STREAM_CURSOR_INVALID",
        ),
        (
            "5",
            Some(HeaderValue::from_static("")),
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_INVALID",
        ),
    ];
    for (query, header_value, status, expected) in cases {
        let mut request = get(&format!(
            "/api/stream/trades?stream_token=x&last_event_id={query}"
        ));
        if let Some(value) = header_value.clone() {
            request.headers_mut().insert("last-event-id", value);
        }
        let context = format!("query {query:?}, header {header_value:?}");
        let (actual, _, bytes) = send(&router, request).await;
        assert_eq!(actual, status, "{context}");
        assert_eq!(code(&bytes), expected, "{context}");
    }
}

/// The first frame of each public stream is its first event: no process
/// diagnostic comment comes before it. The private stream's first frame is
/// checked against the database in `trades_get_alias_matches_canonical`.
#[tokio::test]
async fn no_pid_origin_comment_in_rust_stream() {
    let hub = StreamHub::default();
    let router = live(&hub);
    for (path, channel) in PUBLIC_ROUTES {
        let mut frames = Frames::open(&router, get(path)).await;
        hub.invalidate_public(channel).unwrap();
        let first = frames.next().await;
        assert!(
            !first.starts_with(':'),
            "{path}: a comment came first: {first:?}"
        );
        assert!(!first.to_lowercase().contains("pid"), "{path}: {first:?}");
        assert_control(&first, "market_invalidated", invalidation(), false);
    }
}

#[tokio::test]
async fn stream_error_order_and_envelope() {
    let router = live(&StreamHub::default());
    // Cursor syntax precedes the Authorization conflict.
    let bearer_and_bad_cursor = request(
        "GET",
        "/api/stream/trades?stream_token=x&last_event_id=abc",
        &[("authorization", "bearer a")],
    );
    let (status, _, bytes) = send(&router, bearer_and_bad_cursor).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
    // The conflict precedes the missing token.
    let bearer_only = request("GET", CANONICAL_TRADES, &[("authorization", "bearer a")]);
    let (status, _, bytes) = send(&router, bearer_only).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_AUTH_CONFLICT");
    // The token presence precedes token validity.
    let (status, _, bytes) = send(&router, get("/api/stream/trades?stream_token=")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED");
    // A repeated parameter takes its last value, and every refusal is still
    // the code/message object.
    let repeated = [
        (
            "stream_token=&stream_token=not-a-token",
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_INVALID",
        ),
        (
            "stream_token=not-a-token&stream_token=",
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_REQUIRED",
        ),
        (
            "stream_token=x&last_event_id=5&last_event_id=abc",
            StatusCode::BAD_REQUEST,
            "STREAM_CURSOR_INVALID",
        ),
    ];
    for (query, status, expected) in repeated {
        let (actual, _, bytes) = send(&router, get(&format!("{CANONICAL_TRADES}?{query}"))).await;
        assert_eq!(actual, status, "{query}");
        assert_eq!(code(&bytes), expected, "{query}");
    }
    // Every pre-header error is the code/message object.
    let (_, _, bytes) = send(&router, get(CANONICAL_TRADES)).await;
    let detail: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(detail["detail"]["code"].is_string());
    assert!(detail["detail"]["message"].is_string());
}

/// A full channel and the aggregate subscriber cap both answer 200 with one
/// in-band `connection_capacity` control and then close. The private route's
/// capacity control is checked against the database in
/// `trades_get_alias_matches_canonical`.
#[tokio::test]
async fn capacity_is_terminal_event() {
    for (path, channel) in PUBLIC_ROUTES {
        let hub = hub_with(1, MAX_PROCESS_SUBSCRIBERS);
        let _held = hub.subscribe_public(channel).unwrap();
        assert_capacity_terminal(&live(&hub), get(path)).await;
        assert_eq!(hub.subscriber_count(), 1, "{path}: a refusal registered");
    }
    // The aggregate cap: one process slot, held by the other public channel.
    for (path, channel) in PUBLIC_ROUTES {
        let other = match channel {
            PublicChannel::Prices => PublicChannel::Orderbook,
            PublicChannel::Orderbook => PublicChannel::Prices,
        };
        let hub = hub_with(MAX_CHANNEL_SUBSCRIBERS, 1);
        let _held = hub.subscribe_public(other).unwrap();
        assert_capacity_terminal(&live(&hub), get(path)).await;
        assert_eq!(hub.subscriber_count(), 1, "{path}: a refusal registered");
    }
}

/// An open stream that has nothing to send writes a comment within the
/// 30 second heartbeat bound.
#[tokio::test]
async fn heartbeat_within_bound() {
    let hub = StreamHub::default();
    let router = live(&hub);
    let started = tokio::time::Instant::now();
    let mut prices = Frames::open(&router, get("/api/stream/prices")).await;
    let mut orderbook = Frames::open(&router, get("/api/stream/orderbook")).await;
    let bound = HEARTBEAT_BOUND + TIMER_SLACK;
    let (prices_frame, orderbook_frame) =
        tokio::join!(prices.next_within(bound), orderbook.next_within(bound));
    for frame in [prices_frame, orderbook_frame] {
        let frame = frame.expect("the stream stays open");
        assert!(is_comment(&frame), "{frame:?}");
    }
    assert!(started.elapsed() <= bound);
}

/// Frames are built by the SSE encoder from validated parts: a fixed public
/// payload with no id, and private payload text that cannot end its line or
/// its frame.
#[tokio::test]
async fn safe_event_framing() {
    let hub = StreamHub::default();
    let router = live(&hub);
    for (path, channel) in PUBLIC_ROUTES {
        let mut frames = Frames::open(&router, get(path)).await;
        hub.invalidate_public(channel).unwrap();
        assert_control(
            &frames.next().await,
            "market_invalidated",
            invalidation(),
            false,
        );
    }
    // A private payload whose text holds line breaks and field names is one
    // frame through the private body's encoder: its id, its name and one data
    // line that decodes to the payload without the transport marker. The
    // database case checks the same payload through the route.
    let organization = Uuid::new_v4();
    let event = StoredEvent::new(42, vec![organization], "trade_created", hostile_payload())
        .expect("a valid event");
    let frame: Result<Event, Infallible> = Ok(ReplayFrame::Business(event).into_event());
    let mut frames = Frames::new(Sse::new(futures_util::stream::iter([frame])).into_response());
    assert_eq!(
        business(&frames.next().await),
        (42, "trade_created".to_owned(), hostile_delivered())
    );
    frames.assert_end().await;
    // An event name that could end its line never reaches the encoder.
    for name in [
        "trade_created\nevent: auth_revoked",
        "trade_created\r",
        "trade created",
        "",
    ] {
        assert!(
            StoredEvent::new(1, vec![organization], name, json!({})).is_err(),
            "{name:?}"
        );
    }
}

/// Inside the serving composition a stream answer carries the shared CORS
/// grant and request ID, whether it is a stream or a pre-header refusal.
#[tokio::test]
async fn cors_and_request_id() {
    let hub = StreamHub::default();
    let router = served(offline_state(Some(streams(&hub))));
    // A trusted browser origin reads the stream with credentials and gets
    // its request ID back.
    let response = call(
        &router,
        request(
            "GET",
            "/api/stream/prices",
            &[
                ("origin", TRUSTED_ORIGIN),
                ("x-request-id", "stream-cors-1"),
            ],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    drop(response);
    assert_stream_headers(&headers, "trusted origin");
    assert_eq!(
        header_text(&headers, "access-control-allow-origin"),
        Some(TRUSTED_ORIGIN)
    );
    assert_eq!(
        header_text(&headers, "access-control-allow-credentials"),
        Some("true")
    );
    assert!(header_text(&headers, "vary").is_some_and(|value| value.contains("Origin")));
    assert!(
        header_text(&headers, "access-control-expose-headers")
            .is_some_and(|value| value.contains("X-Request-ID"))
    );
    assert_eq!(header_text(&headers, "x-request-id"), Some("stream-cors-1"));
    // A pre-header refusal carries the same grant, and a request without an
    // ID gets a generated one.
    let (status, headers, bytes) = send(
        &router,
        request("GET", ALIAS_TRADES, &[("origin", TRUSTED_ORIGIN)]),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED");
    assert_eq!(
        header_text(&headers, "access-control-allow-origin"),
        Some(TRUSTED_ORIGIN)
    );
    assert!(header_text(&headers, "x-request-id").is_some_and(|id| Uuid::parse_str(id).is_ok()));
    // An untrusted origin still reads the public stream, without a grant.
    let response = call(
        &router,
        request(
            "GET",
            "/api/stream/orderbook",
            &[("origin", "https://untrusted.example")],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
    drop(response);
    // A preflight is answered by the shared CORS layer, not by the stream
    // method refusal.
    let (status, headers, _) = send(
        &router,
        request(
            "OPTIONS",
            CANONICAL_TRADES,
            &[
                ("origin", TRUSTED_ORIGIN),
                ("access-control-request-method", "GET"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, "access-control-allow-origin"),
        Some(TRUSTED_ORIGIN)
    );
    assert_eq!(hub.subscriber_count(), 0);
}

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// No log line, response header or response body carries a stream token
/// from the query, whether verification refuses it, the store is
/// unreachable after it verifies, or the cursor refusal comes first.
#[test]
fn query_token_not_logged() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    // A current-thread runtime keeps every future on this thread, where the
    // subscriber is installed.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let unsigned = "stream-token-sentinel-4f1c";
    let signed = token(Uuid::new_v4(), Uuid::new_v4());
    runtime.block_on(async {
        let read = lazy_pool(NO_DATABASE);
        read.close().await;
        let router = served(state_with(
            OFFLINE_CONFIG_URL,
            read,
            Some(streams(&StreamHub::default())),
        ));
        let cases = [
            (
                format!("{CANONICAL_TRADES}?stream_token={unsigned}"),
                StatusCode::UNAUTHORIZED,
            ),
            (
                format!("{CANONICAL_TRADES}?stream_token={signed}"),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                format!("{ALIAS_TRADES}?last_event_id=x&stream_token={unsigned}"),
                StatusCode::BAD_REQUEST,
            ),
        ];
        for (path, status) in cases {
            let (actual, headers, body) = send(&router, get(&path)).await;
            assert_eq!(actual, status, "{path}");
            let body = String::from_utf8_lossy(&body);
            let headers = format!("{headers:?}");
            for secret in [unsigned, signed.as_str()] {
                assert!(!body.contains(secret), "the body echoed the token");
                assert!(!headers.contains(secret), "a header echoed the token");
            }
        }
    });
    let captured = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert_eq!(
        captured.matches("\"http_request\"").count(),
        3,
        "every request is logged once: {captured}"
    );
    assert!(!captured.contains(unsigned), "a log line carries the token");
    assert!(!captured.contains(&signed), "a log line carries the token");
    assert!(
        !captured.contains("stream_token"),
        "a log line carries the query"
    );
}

/// A dropped body leaves the hub at once: its registration and its admission
/// slot are both released, so the next request under a one-slot channel cap
/// is a live stream again.
#[tokio::test]
async fn body_drop_cleans_subscription() {
    for (path, channel) in PUBLIC_ROUTES {
        let hub = hub_with(1, MAX_PROCESS_SUBSCRIBERS);
        let router = live(&hub);
        let frames = Frames::open(&router, get(path)).await;
        assert_eq!(hub.subscriber_count(), 1, "{path}");
        drop(frames);
        assert_eq!(hub.subscriber_count(), 0, "{path}: the registration stayed");
        let mut frames = Frames::open(&router, get(path)).await;
        hub.invalidate_public(channel).unwrap();
        assert_control(
            &frames.next().await,
            "market_invalidated",
            invalidation(),
            false,
        );
    }
}

/// The disposable database that `tests/postgres/test_phase6_http.py` seeds:
/// an approved organization with one approved member, a second approved
/// organization, and a pending organization with one approved member. The
/// application role serves the route; the owner role writes outbox rows and
/// changes account state.
mod postgres {
    use super::*;

    pub struct Fixture {
        pub app_url: String,
        pub owner: PgPool,
        pub user: Uuid,
        pub organization: Uuid,
        pub other_organization: Uuid,
        pub pending_user: Uuid,
        pub pending_organization: Uuid,
    }

    fn variable(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| {
            panic!("{name} is required: run tests/postgres/test_phase6_http.py")
        })
    }

    fn id(name: &str) -> Uuid {
        variable(name).parse().expect("a fixture UUID")
    }

    impl Fixture {
        pub async fn connect() -> Self {
            let owner = PgPoolOptions::new()
                .max_connections(2)
                .acquire_timeout(Duration::from_secs(10))
                .connect(&variable("STREAM_HTTP_OWNER_URL"))
                .await
                .expect("owner connection");
            Self {
                app_url: variable("STREAM_HTTP_APP_URL"),
                owner,
                user: id("STREAM_HTTP_USER"),
                organization: id("STREAM_HTTP_ORG"),
                other_organization: id("STREAM_HTTP_OTHER_ORG"),
                pending_user: id("STREAM_HTTP_PENDING_USER"),
                pending_organization: id("STREAM_HTTP_PENDING_ORG"),
            }
        }

        /// A sequence at or above every outbox row and the retention floor,
        /// so rows inserted above it replay in order after it.
        pub async fn baseline(&self) -> i64 {
            sqlx::query_scalar(
                "SELECT GREATEST(\
                 COALESCE((SELECT MAX(stream_seq) FROM public.market_event_outbox), 0), \
                 (SELECT discarded_through_seq FROM public.market_event_replay_state))::bigint",
            )
            .fetch_one(&self.owner)
            .await
            .expect("outbox baseline")
        }

        pub async fn insert(&self, seq: i64, participants: &[Uuid], payload: Value) {
            sqlx::query(
                "INSERT INTO public.market_event_outbox \
                 (id, event_type, aggregate_type, aggregate_id, participant_org_ids, \
                 payload, created_at, stream_seq) \
                 VALUES ($1, 'trade_created', 'trade', 'stream-http-fixture', $2, $3, now(), $4)",
            )
            .bind(Uuid::new_v4())
            .bind(json!(participants))
            .bind(payload)
            .bind(seq)
            .execute(&self.owner)
            .await
            .expect("outbox row");
        }

        pub async fn set_must_change_password(&self, value: bool) {
            sqlx::query("UPDATE public.users SET must_change_password = $2 WHERE id = $1")
                .bind(self.user)
                .bind(value)
                .execute(&self.owner)
                .await
                .expect("account state");
        }
    }
}

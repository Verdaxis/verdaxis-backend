#![cfg(feature = "contract-tests")]

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, Request, request::Builder},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{VerificationSettings, limits::SharedLimits};
use verdaxis_read::commission::{AuditContext, CommissionCommand, CommissionOperation};
use verdaxis_read::market_contract::{locks::LockClass, transaction::MarketCommand};
use verdaxis_read::{
    Config, MarketPolicy,
    commission_routes::{RouteContext, routes},
};

const LIST: &str = "/api/orders/admin/commissions";
const SUMMARY: &str = "/api/orders/admin/commissions/summary";
const SIGNING_KEY: &[u8] = b"commission-test-signing-key-at-least-32-characters";
const TOKEN_PARTY: &str = "commission-test";
const REQUEST_ID: &str = "11111111-2222-4333-8444-555555555555";
/// A documentation address (RFC 5737) for a client that is not a loopback proxy.
const CLIENT: [u8; 4] = [192, 0, 2, 7];
const SUPPORT_HEADER: &str = "x-verdaxis-market-support-context";
/// The production edge accepts bodies up to 32 MB
/// (`tests/fixtures/caddy/production.Caddyfile`).
const EDGE_BODY_LIMIT_BYTES: usize = 32_000_000;

/// Each request waits at most the market lock timeout for a row. On a loaded
/// host the next participant can arrive after a request has stopped waiting;
/// such a round shows no overlap and is repeated.
const OVERLAP_ROUNDS: usize = 20;
const LOCK_WAITERS: &str = "SELECT count(*) FROM pg_stat_activity \
    WHERE wait_event_type='Lock' AND query LIKE '%commissions%' AND pid<>pg_backend_pid()";
const WRITER_WAITERS: &str = "SELECT count(*) FROM pg_stat_activity \
    WHERE wait_event_type='Lock' AND query LIKE '%mark_after_lock%' AND pid<>pg_backend_pid()";

struct Fixture {
    admin_url: String,
    admin: PgConnection,
    pool: sqlx::PgPool,
    actors: [Uuid; 3],
    settings: Arc<VerificationSettings>,
    policy: Arc<MarketPolicy>,
    config: Arc<Config>,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_EFFECTS_ADMIN_DATABASE_URL")
            .expect("explicit migrated disposable URL");
        let runtime_url = std::env::var("RUST_EFFECTS_DATABASE_URL").unwrap();
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert!(a.path().ends_with("_test"));
        assert_eq!(
            (a.host_str(), a.port(), a.path()),
            (r.host_str(), r.port(), r.path())
        );
        assert_ne!(a.username(), r.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert_eq!(revision, "p89_20261022_trade_freeze");
        sqlx::raw_sql(include_str!("fixtures/phase8_commission.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let actors = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        sqlx::query("SELECT commission_fixture_reset($1, $2, $3)")
            .bind(actors[0])
            .bind(actors[1])
            .bind(actors[2])
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&runtime_url)
            .await
            .unwrap();
        let settings = Arc::new(VerificationSettings::new(
            String::from_utf8(SIGNING_KEY.to_vec()).unwrap(),
            None,
            TOKEN_PARTY.into(),
            TOKEN_PARTY.into(),
        ));
        let config: Config = serde_json::from_value(json!({
            "environment": "test",
            "release_sha": "test",
            "migration_revision": "p89_20261022_trade_freeze",
            "database_url": "",
            "db_session_time_zone": "UTC",
            "bind": "127.0.0.1:0",
            "pool_max": 4,
            "python_budget": 0,
            "configured_max_connections": 100
        }))
        .unwrap();
        let policy = serde_json::from_str::<MarketPolicy>(include_str!(
            "../../../deploy/rust/market-policy.json"
        ))
        .unwrap();
        Self {
            admin_url,
            admin,
            pool,
            actors,
            settings,
            policy: Arc::new(policy),
            config: Arc::new(config),
        }
    }
    fn router(&self, permits: Arc<Semaphore>) -> Router {
        routes(Arc::new(RouteContext {
            pool: self.pool.clone(),
            permits,
            settings: self.settings.clone(),
            limits: SharedLimits::default(),
            policy: self.policy.clone(),
            config: self.config.clone(),
        }))
    }
    fn app(&self) -> Router {
        self.router(Arc::new(Semaphore::new(8)))
    }
    fn command(&self, operation: CommissionOperation) -> CommissionCommand {
        CommissionCommand {
            operation,
            policy: self.policy.clone(),
            config: self.config.clone(),
            limits: SharedLimits::default(),
            client: "192.0.2.7".into(),
            at: Utc::now(),
            audit_context: AuditContext::default(),
        }
    }
    async fn seed(&mut self, count: i32) {
        sqlx::query("SELECT commission_fixture_rows($1)")
            .bind(count)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn account(&mut self, actor: usize, status: &str, must_change_password: bool) {
        sqlx::query("SELECT commission_fixture_account($1, $2, $3)")
            .bind(self.actors[actor])
            .bind(status)
            .bind(must_change_password)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    fn token(&self, actor: usize) -> String {
        sign(self.actors[actor], SIGNING_KEY)
    }
    async fn request(&self, actor: usize, method: &str, path: &str, body: Value) -> (u16, Value) {
        request(self.app(), self.token(actor), method, path, body).await
    }
    async fn id(&mut self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM commissions ORDER BY id LIMIT 1")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn stored_status(&mut self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM commissions WHERE id = $1")
            .bind(id)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn updated_at(&mut self, id: Uuid) -> DateTime<Utc> {
        sqlx::query_scalar("SELECT updated_at FROM commissions WHERE id = $1")
            .bind(id)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn audit_changes(&mut self) -> Vec<Value> {
        sqlx::query_scalar("SELECT changes FROM audit_logs ORDER BY timestamp, id")
            .fetch_all(&mut self.admin)
            .await
            .unwrap()
    }
    async fn counts(&mut self) -> (i64, i64, i64) {
        let row = sqlx::query(
            "SELECT (SELECT count(*) FROM audit_logs) AS audits, \
                    (SELECT count(*) FROM notifications) AS notifications, \
                    (SELECT count(*) FROM market_event_outbox) AS events",
        )
        .fetch_one(&mut self.admin)
        .await
        .unwrap();
        (row.get("audits"), row.get("notifications"), row.get("events"))
    }
}

fn sign(subject: Uuid, key: &[u8]) -> String {
    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": subject,
        "type": "access",
        "iat": now,
        "exp": now + 900,
        "iss": TOKEN_PARTY,
        "aud": TOKEN_PARTY
    });
    let key = jsonwebtoken::EncodingKey::from_secret(key);
    jsonwebtoken::encode(&jsonwebtoken::Header::default(), &claims, &key).unwrap()
}

/// A bearer request from a client that is not a loopback proxy.
fn authorized(token: &str, method: &str, path: &str) -> Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .extension(ConnectInfo(SocketAddr::from((CLIENT, 40_000))))
}

async fn send(app: Router, request: Request<Body>) -> (u16, HeaderMap, Value) {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

async fn request(
    app: Router,
    token: String,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let request = authorized(&token, method, path)
        .header("x-request-id", REQUEST_ID)
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _, body) = send(app, request).await;
    (status, body)
}

async fn wait_for_waiters(admin: &mut PgConnection, query: &'static str, expected: i64) -> bool {
    for _ in 0..400 {
        let waiting: i64 = sqlx::query_scalar(query)
            .fetch_one(&mut *admin)
            .await
            .unwrap();
        if waiting >= expected {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

/// A request that stopped waiting at the market lock timeout reports MARKET_BUSY.
fn lock_wait_expired(response: &(u16, Value)) -> bool {
    response.0 == 500 && response.1["detail"]["code"] == "DATABASE_ERROR"
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_is_serialized_and_audited() {
    for _ in 0..OVERLAP_ROUNDS {
        if serialized_patch_round().await {
            return;
        }
    }
    panic!("no round had both administrator connections waiting on the commission row");
}

/// An independent connection holds the commission row while two administrators
/// patch it. Returns false when the two requests did not wait on the row
/// together; every assertion below runs on a round where they did.
async fn serialized_patch_round() -> bool {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("{LIST}/{id}");
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM commissions WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = f.app();
    let token = f.token(0);
    let p = path.clone();
    let first =
        tokio::spawn(
            async move { request(app, token, "PUT", &p, json!({"status":"INVOICED"})).await },
        );
    let first_waiting = wait_for_waiters(&mut f.admin, LOCK_WAITERS, 1).await;
    let app = f.app();
    let token = f.token(1);
    let p = path.clone();
    let second =
        tokio::spawn(async move { request(app, token, "PUT", &p, json!({"status":"PAID"})).await });
    let both_waiting = first_waiting && wait_for_waiters(&mut f.admin, LOCK_WAITERS, 2).await;
    blocker.commit().await.unwrap();
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    for response in [&a, &b] {
        assert!(
            response.0 == 200 || lock_wait_expired(response),
            "unexpected patch response {response:?}"
        );
    }
    if !both_waiting || lock_wait_expired(&a) || lock_wait_expired(&b) {
        return false;
    }
    assert_eq!(a.1["status"], "INVOICED");
    assert_eq!(b.1["status"], "PAID");
    assert_eq!(
        f.audit_changes().await,
        vec![
            json!({"status":{"from":"PENDING","to":"INVOICED"}}),
            json!({"status":{"from":"INVOICED","to":"PAID"}})
        ]
    );
    assert_eq!(f.counts().await, (2, 0, 0));
    let contexts: Vec<Option<String>> = sqlx::query_scalar("SELECT request_id FROM audit_logs")
        .fetch_all(&mut f.admin)
        .await
        .unwrap();
    assert!(
        contexts
            .iter()
            .all(|value| value.as_deref() == Some(REQUEST_ID))
    );
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT action, resource_type, resource_id FROM audit_logs")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert!(rows.iter().all(|r| r
        == &(
            "commission.updated".into(),
            "commission".into(),
            id.to_string()
        )));
    true
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_locks_the_row_in_the_declared_plan() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let [admin, _, buyer] = f.actors;
    let paid = json!({"status":"PAID"});
    let patch = |id: Uuid, body: &Value| CommissionOperation::Patch {
        id,
        body: body.clone(),
    };
    let cases = [
        (
            admin,
            patch(id, &paid),
            vec![(LockClass::User, admin), (LockClass::Commission, id)],
        ),
        (buyer, patch(id, &paid), vec![(LockClass::User, buyer)]),
        (
            admin,
            patch(Uuid::new_v4(), &paid),
            vec![(LockClass::User, admin)],
        ),
        (
            admin,
            patch(id, &json!({"status":"OTHER"})),
            vec![(LockClass::User, admin)],
        ),
        (admin, CommissionOperation::List, vec![(LockClass::User, admin)]),
    ];
    let mut conn = f.pool.acquire().await.unwrap();
    for (actor, operation, expected) in cases {
        let preview = f.command(operation).preview(&mut conn, actor).await.unwrap();
        let expected: Vec<(LockClass, String)> = expected
            .into_iter()
            .map(|(class, id)| (class, id.to_string()))
            .collect();
        assert_eq!(preview.locks.trace(), expected);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_contention_uses_the_database_error_status() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM commissions WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let path = format!("{LIST}/{id}");
    let response = f.request(0, "PUT", &path, json!({"status":"PAID"})).await;
    blocker.rollback().await.unwrap();
    assert_eq!(response.0, 500);
    assert_eq!(response.1["detail"]["code"], "DATABASE_ERROR");
    assert_eq!(f.stored_status(id).await, "PENDING");
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_audit_order_follows_the_row_lock() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("{LIST}/{id}");
    // The first request takes its request time, then waits for a market permit
    // while the second request changes the row and commits.
    let permits = Arc::new(Semaphore::new(1));
    let held = permits.clone().acquire_owned().await.unwrap();
    let app = f.router(permits);
    let token = f.token(0);
    let p = path.clone();
    let first =
        tokio::spawn(
            async move { request(app, token, "PUT", &p, json!({"status":"INVOICED"})).await },
        );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (status, body) = f.request(1, "PUT", &path, json!({"status":"PAID"})).await;
    assert_eq!((status, &body["status"]), (200, &json!("PAID")));
    drop(held);
    let (status, body) = first.await.unwrap();
    assert_eq!((status, &body["status"]), (200, &json!("INVOICED")));
    assert_eq!(
        f.audit_changes().await,
        vec![
            json!({"status":{"from":"PENDING","to":"PAID"}}),
            json!({"status":{"from":"PAID","to":"INVOICED"}})
        ]
    );
    assert_eq!(f.stored_status(id).await, "INVOICED");
    assert_eq!(f.counts().await, (2, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_audit_records_the_client_and_request_context() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let path = format!("{LIST}/{}", f.id().await);
    let long = "a".repeat(64);
    let supplied = [
        None,
        Some(long.as_str()),
        Some("trace id/with space"),
        Some("trace-1.a:b"),
    ];
    for (index, header) in supplied.iter().enumerate() {
        let mut builder = authorized(&f.token(0), "PUT", &path);
        if let Some(value) = header {
            builder = builder.header("x-request-id", *value);
        }
        let body = json!({ "notes": format!("case {index}") }).to_string();
        let request = builder.body(Body::from(body)).unwrap();
        let (status, _, _) = send(f.app(), request).await;
        assert_eq!(status, 200, "request ID {header:?}");
    }
    let rows: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT ip_address, request_id FROM audit_logs ORDER BY timestamp, id")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(rows.len(), supplied.len());
    assert!(rows.iter().all(|(ip, _)| ip.as_deref() == Some("192.0.2.7")));
    for (_, generated) in &rows[..3] {
        let generated = generated.as_deref().unwrap_or_default();
        assert!(Uuid::parse_str(generated).is_ok(), "{generated:?}");
    }
    assert_eq!(rows[3].1.as_deref(), Some("trace-1.a:b"));
    for read in [LIST, SUMMARY] {
        let request = authorized(&f.token(0), "GET", read)
            .header("x-request-id", long.as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(f.app(), request).await.0, 200, "{read}");
    }
    // Behind the serving middleware the audit row keeps the ID of the response.
    let served = verdaxis_read::runtime::http::middleware(f.app());
    let body = json!({"notes":"served"}).to_string();
    let request = authorized(&f.token(0), "PUT", &path)
        .body(Body::from(body))
        .unwrap();
    let (status, headers, _) = send(served, request).await;
    assert_eq!(status, 200);
    let returned = headers["x-request-id"].to_str().unwrap().to_owned();
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT request_id FROM audit_logs ORDER BY timestamp DESC, id DESC LIMIT 1",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some(returned.as_str()));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_support_context_is_refused_only_for_changes() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let context = Uuid::new_v4().to_string();
    for read in [LIST, SUMMARY] {
        let request = authorized(&f.token(0), "GET", read)
            .header(SUPPORT_HEADER, context.as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(f.app(), request).await.0, 200, "{read}");
    }
    let path = format!("{LIST}/{}", f.id().await);
    let request = authorized(&f.token(0), "PUT", &path)
        .header(SUPPORT_HEADER, context.as_str())
        .body(Body::from(json!({"status":"PAID"}).to_string()))
        .unwrap();
    let served = verdaxis_read::runtime::http::middleware(f.app());
    let (status, _, body) = send(served, request).await;
    assert_eq!(status, 403);
    assert_eq!(body["detail"]["code"], "MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_list_is_unbounded_and_preserves_the_array_shape() {
    let mut f = Fixture::new().await;
    assert_eq!(f.request(0, "GET", LIST, Value::Null).await, (200, json!([])));
    f.seed(1007).await;
    let (status, body) = f
        .request(0, "GET", &format!("{LIST}?limit=1&skip=1000"), Value::Null)
        .await;
    assert_eq!(status, 200);
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 1007);
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM commissions ORDER BY created_at DESC, id DESC")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    for row in rows {
        assert_eq!(row.as_object().unwrap().len(), 8);
        assert!(row["amount_usd"].is_string());
        assert!(row["invoice_number"].is_null());
        assert!(row.get("notes").is_none());
    }
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_views_do_not_derive_from_trades() {
    let mut f = Fixture::new().await;
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let product = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK")
        .unwrap();
    sqlx::query("SELECT commission_fixture_trade($1, $2, $3)")
        .bind(product["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .bind(
            product["lanes"][0]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
        )
        .bind(product["code"].as_str().unwrap())
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request(0, "GET", LIST, Value::Null).await, (200, json!([])));
    let (_, body) = f.request(0, "GET", SUMMARY, Value::Null).await;
    assert_eq!(body["paid_count"], 0);
    assert_eq!(
        body["total_paid_usd"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::ZERO
    );
    f.seed(1).await;
    let (_, body) = f.request(0, "GET", LIST, Value::Null).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn summary_totals_are_exact() {
    let mut f = Fixture::new().await;
    f.seed(1007).await;
    sqlx::query("SELECT commission_fixture_amounts()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, body) = f.request(0, "GET", SUMMARY, Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(body.as_object().unwrap().len(), 6);
    for (name, status) in [
        ("pending", "PENDING"),
        ("invoiced", "INVOICED"),
        ("paid", "PAID"),
    ] {
        let row = sqlx::query(
            "SELECT count(*) AS n, coalesce(sum(amount_usd), 0)::text AS total \
             FROM commissions WHERE status = $1",
        )
        .bind(status)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
        assert_eq!(body[format!("{name}_count")], row.get::<i64, _>("n"));
        assert_eq!(
            body[format!("total_{name}_usd")]
                .as_str()
                .unwrap()
                .parse::<rust_decimal::Decimal>()
                .unwrap(),
            row.get::<String, _>("total")
                .parse::<rust_decimal::Decimal>()
                .unwrap()
        );
    }
    assert_eq!(f.counts().await, (0, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_has_no_query_after_commit() {
    for _ in 0..OVERLAP_ROUNDS {
        if response_before_commit_round().await {
            return;
        }
    }
    panic!("no round queued the later writer while the patch held the commission row");
}

/// The patch waits on an independent row lock. A later writer then queues for
/// an exclusive table lock and changes the row as soon as the patch commits.
/// A response read after the commit would wait for that writer and show its
/// value; the response must show the row of the patch's own transaction.
/// Returns false when the patch stopped waiting before the writer queued.
async fn response_before_commit_round() -> bool {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM commissions WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let body = json!({
        "status": "PAID",
        "invoice_number": "A-1",
        "invoice_date": "2026-10-25",
        "payment_date": "2026-10-26",
        "notes": "retained",
        "amount_usd": "1"
    });
    let app = f.app();
    let token = f.token(0);
    let path = format!("{LIST}/{id}");
    let patch = tokio::spawn(async move { request(app, token, "PUT", &path, body).await });
    let patch_waiting = wait_for_waiters(&mut f.admin, LOCK_WAITERS, 1).await;
    let mut writer = PgConnection::connect(&f.admin_url).await.unwrap();
    let later_writer = tokio::spawn(async move {
        sqlx::query("SELECT commission_fixture_mark_after_lock()")
            .execute(&mut writer)
            .await
            .unwrap();
    });
    let writer_queued = patch_waiting && wait_for_waiters(&mut f.admin, WRITER_WAITERS, 1).await;
    blocker.commit().await.unwrap();
    let response = patch.await.unwrap();
    later_writer.await.unwrap();
    assert!(
        response.0 == 200 || lock_wait_expired(&response),
        "unexpected patch response {response:?}"
    );
    if !writer_queued || lock_wait_expired(&response) {
        return false;
    }
    let body = response.1;
    assert_eq!(body["status"], "PAID");
    assert_eq!(body["invoice_number"], "A-1");
    assert_eq!(body["amount_usd"], "123.45");
    assert!(body.get("notes").is_none());
    let stored: Option<String> =
        sqlx::query_scalar("SELECT invoice_number FROM commissions WHERE id = $1")
            .bind(id)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("AFTER-COMMIT"));
    assert_eq!(f.counts().await, (1, 0, 0));
    true
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_noop_and_reverse_status_keep_one_audit() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let path = format!("{LIST}/{}", f.id().await);
    for body in [
        json!({"status":"PAID"}),
        json!({"status":"PENDING"}),
        json!({"status":"PENDING"}),
        json!({"notes":"changed"}),
        json!({}),
    ] {
        assert_eq!(f.request(0, "PUT", &path, body).await.0, 200);
    }
    assert_eq!(
        f.audit_changes().await,
        vec![
            json!({"status":{"from":"PENDING","to":"PAID"}}),
            json!({"status":{"from":"PAID","to":"PENDING"}}),
            json!({}),
            json!({}),
            json!({})
        ]
    );
    assert_eq!(f.counts().await, (5, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_without_a_change_keeps_updated_at() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("{LIST}/{id}");
    let seeded = f.updated_at(id).await;
    for unchanged in [
        json!({}),
        json!({"status":"PENDING"}),
        json!({"invoice_number":null}),
    ] {
        assert_eq!(f.request(0, "PUT", &path, unchanged).await.0, 200);
        assert_eq!(f.updated_at(id).await, seeded);
    }
    assert_eq!(f.request(0, "PUT", &path, json!({"status":"PAID"})).await.0, 200);
    let audited: DateTime<Utc> = sqlx::query_scalar(
        "SELECT timestamp FROM audit_logs ORDER BY timestamp DESC, id DESC LIMIT 1",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(f.updated_at(id).await, audited);
    assert_eq!(f.counts().await, (4, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_admin_authority_precedes_resource_access() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let requests = [
        ("GET", LIST.to_owned()),
        ("GET", SUMMARY.to_owned()),
        ("PUT", format!("{LIST}/{id}")),
        ("PUT", format!("{LIST}/{}", Uuid::new_v4())),
    ];
    for (method, path) in requests {
        let (status, _) = f.request(2, method, &path, json!({"status":"PAID"})).await;
        assert_eq!(status, 403, "{method} {path}");
    }
    assert_eq!(f.counts().await, (0, 0, 0));
    let absent = format!("{LIST}/{}", Uuid::new_v4());
    assert_eq!(f.request(0, "PUT", &absent, json!({})).await.0, 404);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_body_is_validated_before_the_administrator_check() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("{LIST}/{id}");
    let (status, _) = f.request(2, "PUT", &path, json!({"status":"OTHER"})).await;
    assert_eq!(status, 422);
    for invalid in [
        json!({"status":null}),
        json!({"invoice_number":"x".repeat(51)}),
        json!({"notes":"n".repeat(501)}),
    ] {
        assert_eq!(f.request(0, "PUT", &path, invalid).await.0, 500);
    }
    assert_eq!(f.request(2, "PUT", &path, json!({"status":null})).await.0, 403);
    assert_eq!(f.stored_status(id).await, "PENDING");
    assert_eq!(f.counts().await, (0, 0, 0));
    let longest = json!({"invoice_number":"x".repeat(50),"notes":"n".repeat(500)});
    assert_eq!(f.request(0, "PUT", &path, longest).await.0, 200);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_requires_an_approved_administrator_session() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let path = format!("{LIST}/{}", f.id().await);
    let forged = sign(
        f.actors[0],
        b"a-different-signing-key-of-at-least-32-characters",
    );
    assert_eq!(request(f.app(), forged, "GET", LIST, Value::Null).await.0, 401);
    for (status, must_change_password) in [("PENDING", false), ("APPROVED", true)] {
        f.account(0, status, must_change_password).await;
        assert_eq!(f.request(0, "GET", LIST, Value::Null).await.0, 403);
        let (refused, _) = f.request(0, "PUT", &path, json!({"status":"PAID"})).await;
        assert_eq!(refused, 403, "{status} {must_change_password}");
    }
    assert_eq!(f.counts().await, (0, 0, 0));
    f.account(0, "APPROVED", false).await;
    assert_eq!(f.request(0, "PUT", &path, json!({"status":"PAID"})).await.0, 200);
    assert_eq!(f.counts().await, (1, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_patch_body_limit_matches_the_edge() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("{LIST}/{id}");
    // Unknown keys are ignored, so a large body the edge passes is decoded.
    let padded = json!({"status":"PAID","padding":"p".repeat(2 * 1024 * 1024)});
    assert_eq!(f.request(0, "PUT", &path, padded).await.0, 200);
    let oversized = json!({"status":"INVOICED","padding":"p".repeat(EDGE_BODY_LIMIT_BYTES)});
    let (status, body) = f.request(0, "PUT", &path, oversized).await;
    assert_eq!(status, 413);
    assert_eq!(body["detail"]["code"], "BODY_TOO_LARGE");
    let chunks = [
        Ok::<_, std::io::Error>(Bytes::from_static(b"{\"status\":")),
        Err(std::io::Error::other("connection reset")),
    ];
    let broken = Body::from_stream(futures_util::stream::iter(chunks));
    let request = authorized(&f.token(0), "PUT", &path).body(broken).unwrap();
    assert_eq!(send(f.app(), request).await.0, 400);
    assert_eq!(f.stored_status(id).await, "PAID");
    assert_eq!(f.counts().await, (1, 0, 0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commission_update_and_audit_rollback_together() {
    let mut f = Fixture::new().await;
    f.seed(1).await;
    let id = f.id().await;
    let path = format!("{LIST}/{id}");
    sqlx::query("SELECT commission_fixture_refuse_audit(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request(0, "PUT", &path, json!({"status":"PAID"})).await.0, 500);
    assert_eq!(f.stored_status(id).await, "PENDING");
    assert_eq!(f.counts().await, (0, 0, 0));
    sqlx::query("SELECT commission_fixture_refuse_audit(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.request(0, "PUT", &path, json!({"status":"PAID"})).await.0, 200);
    assert_eq!(f.counts().await, (1, 0, 0));
}

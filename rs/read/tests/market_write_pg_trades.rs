#![cfg(feature = "contract-tests")]
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::trades::contract_router;
const KEY: &str = "synthetic-trades-signing-key-at-least-32";
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
const ORDER: u128 = 1;
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const MEMBER: u128 = 0x1002;
const STRANGER: u128 = 0x3001;
const ADMIN: u128 = 0x4001;
struct Fixture {
    admin: PgConnection,
    pool: PgPool,
    router: Router,
}
impl Fixture {
    async fn new() -> Self {
        let a = std::env::var("RUST_TRADES_ADMIN_DATABASE_URL").expect("disposable admin URL");
        let r = std::env::var("RUST_TRADES_DATABASE_URL").expect("disposable runtime URL");
        let au = url::Url::parse(&a).unwrap();
        let ru = url::Url::parse(&r).unwrap();
        assert_eq!(au.host_str(), Some("127.0.0.1"));
        assert!(au.path().ends_with("_test"));
        assert_eq!(
            (au.host_str(), au.port(), au.path()),
            (ru.host_str(), ru.port(), ru.path())
        );
        assert_ne!(au.username(), ru.username());
        let mut admin = PgConnection::connect(&a).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase9_trades.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let policy: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let p = policy["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["qualification_profile"] == "scalar"
                    && p["execution_mode"] == "ORDERBOOK"
                    && p.get("specification_standard").is_none()
            })
            .unwrap();
        sqlx::query("SELECT trades_fixture_seed($1,$2)")
            .bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap())
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&r)
            .await
            .unwrap();
        let router = contract_router(pool.clone(), KEY, true);
        Self {
            admin,
            pool,
            router,
        }
    }
    async fn digest(&mut self) -> String {
        let value: Value =
            sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
                .bind(id(ORDER))
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
        verdaxis_read::canonical_terms_from_row(&value)
            .and_then(|v| verdaxis_read::digest_terms(&v))
            .unwrap()
    }
    async fn take(
        &mut self,
        actor: u128,
        key: Option<&str>,
        quantity: &str,
    ) -> (StatusCode, Value) {
        let digest = self.digest().await;
        let headers: Vec<_> = key
            .map(|k| vec![("idempotency-key", k)])
            .unwrap_or_default();
        send(
            &self.router,
            "POST",
            "/api/trades/",
            Some(actor),
            &headers,
            Some(
                json!({"order_id":id(ORDER),"quantity_mt":quantity,"expected_terms_digest":digest}),
            ),
        )
        .await
    }
    async fn state(&mut self) -> Value {
        sqlx::query_scalar("SELECT trades_fixture_state()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn change(&mut self, target: u128, change: &str) {
        sqlx::query("SELECT trades_fixture_change($1,$2)")
            .bind(id(target))
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
}
async fn send(
    router: &Router,
    method: &str,
    path: &str,
    actor: Option<u128>,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(actor) = actor {
        let now = chrono::Utc::now().timestamp();
        let token=encode(&Header::default(),&json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":"trades-tests","aud":"trades-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = router
        .clone()
        .oneshot(
            request
                .header("content-type", "application/json")
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn take_inserts_pending_without_consuming_stock() {
    let mut f = Fixture::new().await;
    let (s, b) = f.take(BUYER, Some("one"), "3").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "PENDING_CONFIRMATION");
    assert!(b["seller_id"].is_null());
    let state = f.state().await;
    assert_eq!(state["remaining"], "7.00");
    assert_eq!(state["reserved"], "10.00");
    assert_eq!(state["current"], "0.00");
    assert_eq!(state["trades"], 1);
    assert_eq!(state["audit"], 1);
    assert_eq!(state["notifications"], 1);
    assert_eq!(state["outbox"], 1);
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn take_refusal_matrix() {
    let mut f = Fixture::new().await;
    let before = f.state().await;
    let (s, b) = f.take(SELLER, None, "1").await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "supplier-hits-ASK must be 403: {b}"
    );
    assert_eq!(f.state().await, before);
    for (actor, change, status) in [
        (BUYER, "pending", 403),
        (BUYER, "email", 403),
        (BUYER, "password", 403),
        (BUYER, "kyc", 403),
        (SELLER, "pending", 400),
        (ORDER, "expired", 400),
        (ORDER, "off_spec", 400),
        (ORDER, "cancelled", 400),
    ] {
        let mut f = Fixture::new().await;
        f.change(actor, change).await;
        let before = f.state().await;
        let (s, b) = f.take(BUYER, None, "1").await;
        assert_eq!(s.as_u16(), status, "{change}: {b}");
        assert_eq!(f.state().await, before);
    }
    let mut f = Fixture::new().await;
    let before = f.state().await;
    let (s, _) = f.take(BUYER, None, "11").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(f.state().await, before);
    let (s, b) = send(
        &f.router,
        "POST",
        "/api/trades/",
        Some(BUYER),
        &[],
        Some(json!({"order_id":id(ORDER),"quantity_mt":"1"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["detail"]["code"], "ORDER_TERMS_REVIEW_REQUIRED");
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn support_header_is_refused_before_take_validation() {
    let f = Fixture::new().await;
    for value in ["bad", "00000000-0000-0000-0000-000000006000"] {
        let (s, b) = send(
            &f.router,
            "POST",
            "/api/trades/",
            Some(BUYER),
            &[("x-verdaxis-market-support-context", value)],
            Some(json!({})),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert_eq!(b["detail"]["code"], "MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
    }
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn take_replay_returns_the_stored_projection() {
    let mut f = Fixture::new().await;
    let (s, first) = f.take(BUYER, Some("replay"), "2").await;
    assert_eq!(s, StatusCode::OK);
    let before = f.state().await;
    let (s, replay) = f.take(BUYER, Some("replay"), "2").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(first, replay);
    assert_eq!(f.state().await, before);
    assert_eq!(
        f.take(MEMBER, Some("replay"), "2").await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.take(BUYER, Some("replay"), "3").await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.state().await, before);
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn take_freezes_the_fee_basis() {
    let mut f = Fixture::new().await;
    let (s, b) = f.take(BUYER, None, "2").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let row=sqlx::query("SELECT commission_fee_per_mt_usd::text AS fee,commission_plan,commission_rate_pct::text AS legacy FROM trades").fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String, _>("fee"), "2.00");
    assert_eq!(row.get::<String, _>("commission_plan"), "FREE");
    assert_eq!(row.get::<String, _>("legacy"), "0.000");
    assert!(b["commission_fee_per_mt_usd"].is_null());
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn detached_participant_keeps_only_recorded_history() {
    let mut f = Fixture::new().await;
    assert_eq!(f.take(BUYER, None, "2").await.0, StatusCode::OK);
    f.change(BUYER, "detached").await;
    let (s, b) = send(&f.router, "GET", "/api/trades/my", Some(BUYER), &[], None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["total"], 1);
    assert_eq!(b["items"][0]["buyer_id"], id(0x1000).to_string());
    let (_, b) = send(
        &f.router,
        "GET",
        "/api/trades/summary",
        Some(BUYER),
        &[],
        None,
    )
    .await;
    assert_eq!(b["total_count"], 1);
    assert_eq!(b["awaiting_counterparty_count"], 1);
    assert_eq!(b["action_required_count"], 0);
    let (_, b) = send(
        &f.router,
        "GET",
        "/api/trades/my",
        Some(STRANGER),
        &[],
        None,
    )
    .await;
    assert_eq!(b["total"], 0);
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn rejected_participant_keeps_discovery_without_confirmation_action() {
    let mut f = Fixture::new().await;
    assert_eq!(f.take(BUYER, None, "2").await.0, StatusCode::OK);
    f.change(SELLER, "rejected").await;
    let (s, b) = send(
        &f.router,
        "GET",
        "/api/trades/summary",
        Some(SELLER),
        &[],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["total_count"], 1);
    assert_eq!(b["action_required_count"], 0);
    let (_, b) = send(
        &f.router,
        "GET",
        "/api/trades/my?action_required=true",
        Some(SELLER),
        &[],
        None,
    )
    .await;
    assert_eq!(b["total"], 0);
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn moved_participant_view_uses_recorded_side() {
    let mut f = Fixture::new().await;
    assert_eq!(f.take(BUYER, None, "2").await.0, StatusCode::OK);
    f.change(BUYER, "moved").await;
    let (_, b) = send(&f.router, "GET", "/api/trades/my", Some(BUYER), &[], None).await;
    assert_eq!(b["total"], 1);
    assert_eq!(b["items"][0]["buyer_id"], id(0x1000).to_string());
    assert!(b["items"][0]["seller_id"].is_null());
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn trade_reads_are_private_and_stable() {
    let mut f = Fixture::new().await;
    assert_eq!(f.take(BUYER, None, "1").await.0, StatusCode::OK);
    assert_eq!(f.take(BUYER, None, "1").await.0, StatusCode::OK);
    let before = f.state().await;
    let (_, b) = send(
        &f.router,
        "GET",
        "/api/trades/my?limit=1",
        Some(BUYER),
        &[],
        None,
    )
    .await;
    let first = b["items"][0]["id"].clone();
    assert_eq!(b["total"], 2);
    let (_, b) = send(
        &f.router,
        "GET",
        "/api/trades/my?limit=1&skip=1",
        Some(BUYER),
        &[],
        None,
    )
    .await;
    assert_ne!(first, b["items"][0]["id"]);
    let (_, b) = send(
        &f.router,
        "GET",
        "/api/trades/summary",
        Some(SELLER),
        &[],
        None,
    )
    .await;
    assert_eq!(b["total_count"], 2);
    assert_eq!(b["action_required_count"], 2);
    let (_, b) = send(&f.router, "GET", "/api/trades/my", Some(MEMBER), &[], None).await;
    assert_eq!(b["total"], 2);
    assert_eq!(f.state().await, before);
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn invalid_support_context_never_falls_back_to_history() {
    let mut f = Fixture::new().await;
    assert_eq!(f.take(BUYER, None, "1").await.0, StatusCode::OK);
    for context in ["bad", "00000000-0000-0000-0000-000000006000"] {
        let (s, b) = send(
            &f.router,
            "GET",
            "/api/trades/my",
            Some(BUYER),
            &[("x-verdaxis-market-support-context", context)],
            None,
        )
        .await;
        assert!(matches!(s.as_u16(), 400 | 404));
        assert_eq!(b["detail"]["code"], "MARKET_SUPPORT_CONTEXT_INVALID");
    }
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn expired_support_context_commits_its_shared_transition() {
    let mut f = Fixture::new().await;
    let context: Uuid = sqlx::query_scalar("SELECT trades_fixture_support(true)")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let (s, b) = send(
        &f.router,
        "GET",
        "/api/trades/my",
        Some(ADMIN),
        &[("x-verdaxis-market-support-context", &context.to_string())],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    let row=sqlx::query("SELECT status,version,ended_at IS NOT NULL AS ended FROM market_support_contexts WHERE id=$1").bind(context).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String, _>("status"), "EXPIRED");
    assert_eq!(row.get::<i32, _>("version"), 2);
    assert!(row.get::<bool, _>("ended"));
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn supported_reads_use_only_the_bound_customer() {
    let mut f = Fixture::new().await;
    assert_eq!(f.take(BUYER, None, "1").await.0, StatusCode::OK);
    let context: Uuid = sqlx::query_scalar("SELECT trades_fixture_support(false)")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let (s, b) = send(
        &f.router,
        "GET",
        "/api/trades/my",
        Some(ADMIN),
        &[("x-verdaxis-market-support-context", &context.to_string())],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["total"], 1);
    assert_eq!(b["items"][0]["buyer_id"], id(0x1000).to_string());
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn concurrent_takes_preserve_the_pending_reservation() {
    let mut f = Fixture::new().await;
    let digest = f.digest().await;
    let body = json!({"order_id":id(ORDER),"quantity_mt":"7","expected_terms_digest":digest});
    let (a, b) = tokio::join!(
        send(
            &f.router,
            "POST",
            "/api/trades/",
            Some(BUYER),
            &[],
            Some(body.clone())
        ),
        send(
            &f.router,
            "POST",
            "/api/trades/",
            Some(STRANGER),
            &[],
            Some(body)
        )
    );
    let successes = [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(successes, 1, "{a:?} {b:?}");
    let state = f.state().await;
    assert_eq!(state["remaining"], "3.00");
    assert_eq!(state["reserved"], "10.00");
    assert_eq!(state["trades"], 1);
    assert!(f.pool.size() > 0);
}

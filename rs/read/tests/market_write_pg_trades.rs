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
    async fn take(&mut self, actor:u128,key:Option<&str>,quantity:&str)->(StatusCode,Value){
        let digest=if let Some(key)=key {
            let stored:Option<String>=sqlx::query_scalar("SELECT value FROM trades_fixture_requests WHERE key=$1").bind(key).fetch_optional(&mut self.admin).await.unwrap();
            if let Some(value)=stored {value}else{let digest=self.digest().await;sqlx::query("INSERT INTO trades_fixture_requests(key,value) VALUES($1,$2)").bind(key).bind(&digest).execute(&mut self.admin).await.unwrap();digest}
        }else{self.digest().await};
        let headers:Vec<_>=key.map(|k|vec![("idempotency-key",k)]).unwrap_or_default();
        send(&self.router,"POST","/api/trades/",Some(actor),&headers,Some(json!({"order_id":id(ORDER),"quantity_mt":quantity,"expected_terms_digest":digest}))).await
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
    assert_eq!(state["remaining"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(7u32));
    assert_eq!(state["reserved"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(10u32));
    assert_eq!(state["current"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(0u32));
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
    assert_eq!(row.get::<String, _>("fee").parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(2));
    assert_eq!(row.get::<String, _>("commission_plan"), "free");
    assert_eq!(row.get::<String, _>("legacy").parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::ZERO);
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
    assert_eq!(state["remaining"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(3u32));
    assert_eq!(state["reserved"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(10u32));
    assert_eq!(state["trades"], 1);
    f.pool.acquire().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn take_and_cancel_release_only_the_uncommitted_remainder() {
    let mut f=Fixture::new().await;
    assert_eq!(f.take(BUYER,None,"3").await.0,StatusCode::OK);
    let cancel=verdaxis_read::routes::market_write::orders::contract_router(f.pool.clone(),KEY);
    // Cancellation's component uses its own token audience.
    let now=chrono::Utc::now().timestamp();
    let token=encode(&Header::default(),&json!({"sub":id(SELLER),"type":"access","iat":now,"exp":now+600,"iss":"cancel-tests","aud":"cancel-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let response=cancel.oneshot(Request::builder().method("DELETE").uri(format!("/api/orderbook/{}",id(ORDER))).header("authorization",format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(),StatusCode::NO_CONTENT);
    let state=f.state().await;
    assert_eq!(state["reserved"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(3));
    assert_eq!(state["current"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap(),rust_decimal::Decimal::from(7));
    assert_eq!(state["trades"],1);
    let before=state;
    assert_eq!(f.take(STRANGER,None,"1").await.0,StatusCode::BAD_REQUEST);
    assert_eq!(f.state().await,before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn summary_and_list_deduplicate_current_and_recorded_membership() {
    let mut f=Fixture::new().await;
    assert_eq!(f.take(BUYER,None,"1").await.0,StatusCode::OK);
    let(_,list)=send(&f.router,"GET","/api/trades/my",Some(BUYER),&[],None).await;
    let(_,summary)=send(&f.router,"GET","/api/trades/summary",Some(BUYER),&[],None).await;
    assert_eq!(list["total"],1);assert_eq!(list["items"].as_array().unwrap().len(),1);assert_eq!(summary["total_count"],1);assert_eq!(summary["awaiting_counterparty_count"],1);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn version_is_hashed_without_a_fresh_version_mismatch_refusal() {
    let mut f=Fixture::new().await;let digest=f.digest().await;
    let(s,b)=send(&f.router,"POST","/api/trades/",Some(BUYER),&[],Some(json!({"order_id":id(ORDER),"quantity_mt":"1","expected_terms_digest":digest,"expected_order_version":999}))).await;
    assert_eq!(s,StatusCode::OK,"{b}");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn exact_key_replay_precedes_current_kyc_and_uses_current_trade_projection() {
    let mut f=Fixture::new().await;let(s,trade)=f.take(BUYER,Some("current"),"1").await;assert_eq!(s,StatusCode::OK);
    let trade_id=trade["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    sqlx::query("SELECT trades_fixture_confirm($1)").bind(trade_id).execute(&mut f.admin).await.unwrap();
    f.change(BUYER,"kyc").await;let before=f.state().await;
    let(s,replay)=f.take(BUYER,Some("current"),"1").await;assert_eq!(s,StatusCode::OK,"{replay}");assert_eq!(replay["status"],"CONFIRMED");assert_eq!(replay["seller_id"],id(0x2000).to_string());assert_eq!(f.state().await,before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn failed_effect_append_rolls_back_the_trade_and_remainder() {
    let mut f=Fixture::new().await;let before=f.state().await;
    sqlx::query("SELECT trades_fixture_reject_audit()").execute(&mut f.admin).await.unwrap();
    assert!(!f.take(BUYER,None,"1").await.0.is_success());
    assert_eq!(f.state().await,before);
    sqlx::query("SELECT trades_fixture_allow_audit()").execute(&mut f.admin).await.unwrap();
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn independent_take_and_cancel_serialize_on_the_held_party_prefix() {
    let mut f=Fixture::new().await;
    let digest=f.digest().await;
    let mut blocker=f.admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE").bind(id(0x2000)).execute(&mut *blocker).await.unwrap();
    let take_router=f.router.clone();let cancel_router=verdaxis_read::routes::market_write::orders::contract_router(f.pool.clone(),KEY);
    let take=tokio::spawn(async move{send(&take_router,"POST","/api/trades/",Some(BUYER),&[],Some(json!({"order_id":id(ORDER),"quantity_mt":"3","expected_terms_digest":digest}))).await});
    let cancel=tokio::spawn(async move{
        let now=chrono::Utc::now().timestamp();let token=encode(&Header::default(),&json!({"sub":id(SELLER),"type":"access","iat":now,"exp":now+600,"iss":"cancel-tests","aud":"cancel-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        cancel_router.oneshot(Request::builder().method("DELETE").uri(format!("/api/orderbook/{}",id(ORDER))).header("authorization",format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap().status()
    });
    let admin_url=std::env::var("RUST_TRADES_ADMIN_DATABASE_URL").unwrap();let mut observer=PgConnection::connect(&admin_url).await.unwrap();
    let observed=tokio::time::timeout(std::time::Duration::from_secs(4),async{
        loop {let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND usename='rust_app'").fetch_one(&mut observer).await.unwrap();if n>=2 {break n;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
    }).await;
    blocker.commit().await.unwrap();assert!(observed.is_ok(),"both independent owners must reach the held prefix");
    let take=take.await.unwrap();let cancel=cancel.await.unwrap();
    assert!(matches!(take.0.as_u16(),200|400|409|503),"{take:?}");assert!(matches!(cancel.as_u16(),204|409|503));
    let state=f.state().await;let pending=state["trades"].as_i64().unwrap();assert!(pending<=1);
    let reserved=state["reserved"].as_str().unwrap().parse::<rust_decimal::Decimal>().unwrap();
    if cancel==StatusCode::NO_CONTENT {assert_eq!(reserved,rust_decimal::Decimal::from(3*pending));}else{assert_eq!(reserved,rust_decimal::Decimal::from(10));}
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn assisted_discovery_requires_linked_customer_membership_not_staff_identity() {
    let mut f=Fixture::new().await;
    sqlx::query("SELECT trades_fixture_assisted()").execute(&mut f.admin).await.unwrap();
    let(s,b)=f.take(BUYER,None,"1").await;assert_eq!(s,StatusCode::OK,"{b}");
    let(_,customer)=send(&f.router,"GET","/api/trades/summary",Some(SELLER),&[],None).await;assert_eq!(customer["total_count"],1);assert_eq!(customer["action_required_count"],1);
    let(_,staff)=send(&f.router,"GET","/api/trades/my",Some(ADMIN),&[],None).await;assert_eq!(staff["total"],0);
    f.change(SELLER,"detached").await;
    let(_,detached)=send(&f.router,"GET","/api/trades/my",Some(SELLER),&[],None).await;assert_eq!(detached["total"],0);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn equal_time_pages_use_uuid_descending_and_match_summary() {
    let mut f=Fixture::new().await;let(_,a)=f.take(BUYER,None,"1").await;let(_,b)=f.take(BUYER,None,"1").await;
    sqlx::query("SELECT trades_fixture_tie_times()").execute(&mut f.admin).await.unwrap();
    let(_,page)=send(&f.router,"GET","/api/trades/my?limit=1",Some(BUYER),&[],None).await;
    let mut ids=[a["id"].as_str().unwrap(),b["id"].as_str().unwrap()];ids.sort_unstable();
    assert_eq!(page["items"][0]["id"],ids[1]);
    let(_,next)=send(&f.router,"GET","/api/trades/my?limit=1&skip=1",Some(BUYER),&[],None).await;assert_eq!(next["items"][0]["id"],ids[0]);
    let(_,completed)=send(&f.router,"GET","/api/trades/my?status_group=completed",Some(BUYER),&[],None).await;assert_eq!(completed["total"],0);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn pending_effects_have_exact_participant_scope_and_source_fields() {
    let mut f=Fixture::new().await;let(s,trade)=f.take(BUYER,None,"2").await;assert_eq!(s,StatusCode::OK);
    let row=sqlx::query("SELECT event_type,participant_org_ids,payload,producer_evidence FROM market_event_outbox").fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String,_>("event_type"),"trade_created");let mut parties:Vec<Uuid>=serde_json::from_value(row.get::<Value,_>("participant_org_ids")).unwrap();parties.sort_unstable();assert_eq!(parties,vec![id(0x1000),id(0x2000)]);
    let payload:Value=row.get("payload");assert_eq!(payload["id"],trade["id"]);assert_eq!(payload["status"],"PENDING_CONFIRMATION");assert_eq!(payload["_public_market_invalidation"],true);assert_eq!(payload["schema_version"],1);assert!(payload.get("commission_fee_per_mt_usd").is_none());assert!(payload.get("producer_evidence").is_none());
    let audit=sqlx::query("SELECT action,resource_type,changes FROM audit_logs").fetch_one(&mut f.admin).await.unwrap();assert_eq!(audit.get::<String,_>("action"),"trade.created");assert_eq!(audit.get::<String,_>("resource_type"),"trade");let changes:Value=audit.get("changes");assert_eq!(changes["order_id"],id(ORDER).to_string());assert!(changes.get("lng_terms_snapshot").is_none());
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn typed_direct_takes_freeze_normalized_terms_and_keep_pending_identity_private() {
    let fame_bid=json!({"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU"});
    let fame_ask=json!({"schema_version":1,"side":"ASK","neat_fame":true,"uco_mass_pct":100,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU","certificate_reference":"private-certificate","certificate_holder":"private-supplier","evidence_status":"PENDING","document_references":[],"certificate_valid_until":"2027-01-01","evidence_due":"BEFORE_LOADING"});
    let lng_bid=json!({"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_delivery_methods":["SHIP_TO_SHIP"]});
    let lng_ask=json!({"side":"ASK","sustainability_scheme":"ISCC_EU","scheme_certificate_number":"private-certificate","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"100","feedstock":"MANURE","production_country":"NL","wtt_ghg_gco2e_mj":"-20.25","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"physical_supplier_name":"private-supplier","pos_timing":"WITH_DELIVERY"});
    let policy:Value=serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    for(profile,bid,ask,field) in [("fame_v1",fame_bid,fame_ask,"fame_terms"),("lng_v1",lng_bid,lng_ask,"lng_terms")] {
        let mut f=Fixture::new().await;
        let p=policy["products"].as_array().unwrap().iter().find(|p|p["qualification_profile"]==profile).unwrap();
        sqlx::query("SELECT trades_fixture_typed($1,$2,$3,$4)").bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap()).bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap()).bind(profile).bind(&ask).execute(&mut f.admin).await.unwrap();
        let digest=f.digest().await;let mut body=json!({"order_id":id(ORDER),"quantity_mt":"1","expected_terms_digest":digest});body[field]=bid;
        let(s,response)=send(&f.router,"POST","/api/trades/",Some(BUYER),&[],Some(body)).await;assert_eq!(s,StatusCode::OK,"{profile}: {response}");
        let text=response.to_string();assert!(!text.contains("private-certificate"));assert!(!text.contains("private-supplier"));
        let stored:Value=sqlx::query_scalar("SELECT COALESCE(fame_terms_snapshot::jsonb,lng_terms_snapshot::jsonb) FROM trades").fetch_one(&mut f.admin).await.unwrap();
        assert!(stored.to_string().contains("private-certificate"));
        let normalized=verdaxis_read::market::terms::owner_response_terms(profile,&ask).unwrap();assert_eq!(stored["ask"],normalized);
        if profile=="lng_v1" {assert_eq!(stored["bid"]["accept_resellers"],true);assert!(stored["bid"].get("schema_version").is_none());}
    }
}

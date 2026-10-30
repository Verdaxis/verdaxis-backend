#![cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::trade_lifecycle::contract_router;

const KEY: &str = "synthetic-settlement-signing-key-at-least-32";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn delivery() -> Value {
    json!({"final_quantity_mt":"3","final_price_per_mt":"100"})
}
struct Fixture {
    _database: std::sync::Arc<market_database::MarketDatabase>,
    admin: PgConnection,

    router: Router,
    trade: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let database =
            std::sync::Arc::new(market_database::MarketDatabase::new("RUST_SETTLEMENT").await);
        let a = database.admin_url.clone();
        let r = database.runtime_url.clone();
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
        for sql in [
            include_str!("fixtures/phase9_trades.sql"),
            include_str!("fixtures/phase9_confirm.sql"),
            include_str!("fixtures/phase9_settlement.sql"),
            include_str!("fixtures/phase9_trade_membership.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
        }
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
        sqlx::query("SELECT set_config('confirm.policy',$1,false)")
            .bind(policy["products"].to_string())
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT confirm_fixture_pending('BUYER',false)")
            .execute(&mut admin)
            .await
            .unwrap();
        let trade = sqlx::query_scalar("SELECT settlement_fixture_confirmed()")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&r)
            .await
            .unwrap();
        let router = contract_router(pool.clone(), KEY, true).merge(
            verdaxis_read::routes::market_write::trades::contract_router(pool.clone(), KEY, true),
        );
        Self {
            _database: database,
            admin,
            router,
            trade,
        }
    }
    async fn change(&mut self, actor: u128, change: &str) {
        sqlx::query("SELECT CASE WHEN $2='org_reject' THEN settlement_fixture_reject_org($1) ELSE trades_fixture_change($1,$2) END")
            .bind(id(actor))
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn read(&self, actor: u128, summary: bool) -> (StatusCode, Value) {
        let now = chrono::Utc::now().timestamp();
        let token = encode(&Header::default(), &json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":"trades-tests","aud":"trades-tests"}), &EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(if summary {
                        "/api/trades/summary"
                    } else {
                        "/api/trades/my"
                    })
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT confirm_fixture_snapshot()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn call(
        &self,
        action: &str,
        actor: u128,
        key: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let headers = key
            .map(|k| vec![("idempotency-key", k)])
            .unwrap_or_default();
        send(
            &self.router,
            self.trade,
            action,
            Some(actor),
            &headers,
            body.to_string(),
            0,
        )
        .await
    }
    async fn deliver(&self, actor: u128, key: Option<&str>) -> (StatusCode, Value) {
        self.call("deliver", actor, key, delivery()).await
    }
    async fn pay(&self, actor: u128, key: Option<&str>) -> (StatusCode, Value) {
        self.call("pay", actor, key, Value::Null).await
    }
}
async fn send(
    router: &Router,
    trade: Uuid,
    action: &str,
    actor: Option<u128>,
    headers: &[(&str, &str)],
    body: String,
    issued_offset: i64,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(if action == "pay" { "POST" } else { "PUT" })
        .uri(format!("/api/trades/{trade}/{action}"));
    if let Some(actor) = actor {
        let now = chrono::Utc::now().timestamp();
        let token=encode(&Header::default(),&json!({"sub":id(actor),"type":"access","iat":now+issued_offset,"exp":if issued_offset < -1000 {now-1}else{now+600},"iss":"lifecycle-tests","aud":"lifecycle-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
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
async fn ok(result: (StatusCode, Value), status: &str) -> Value {
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["status"], status);
    result.1
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn moved_recorded_seller_cannot_deliver() {
    let mut f = Fixture::new().await;
    f.change(SELLER, "moved").await;
    let before = f.snapshot().await;
    for key in [None, Some("delivery")] {
        let (status, body) = f.deliver(SELLER, key).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.get("status").is_none());
        assert!(body.get("freeze").is_none());
        assert_eq!(f.snapshot().await, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn ordinary_membership_controls_settlement_and_receipt_disclosure() {
    for actor in [BUYER, SELLER] {
        for change in ["detached", "moved", "rejected", "org_reject"] {
            let mut f = Fixture::new().await;
            let delivered = ok(f.deliver(actor, Some("delivery")).await, "DELIVERED").await;
            let paid = ok(f.pay(SELLER, Some("payment")).await, "PAID").await;
            f.change(actor, change).await;
            if change == "org_reject" {
                f.change(actor, "detached").await;
            }
            let before = f.snapshot().await;
            let result = f.deliver(actor, Some("delivery")).await;
            if matches!(change, "detached" | "moved") {
                assert_eq!(result.0, StatusCode::FORBIDDEN, "{change}: {}", result.1);
                assert!(result.1.get("status").is_none());
                if actor == SELLER {
                    assert_eq!(f.pay(actor, Some("payment")).await.0, StatusCode::FORBIDDEN);
                }
            } else {
                assert_eq!(ok(result, "DELIVERED").await, delivered);
                if actor == SELLER {
                    assert_eq!(ok(f.pay(actor, Some("payment")).await, "PAID").await, paid);
                }
            }
            assert_eq!(f.snapshot().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn detached_recorded_members_settle_only_while_their_organization_is_rejected() {
    for actor in [BUYER, SELLER] {
        let mut f = Fixture::new().await;
        f.change(actor, "detached").await;
        let before = f.snapshot().await;
        assert_eq!(f.deliver(actor, None).await.0, StatusCode::FORBIDDEN);
        assert_eq!(f.snapshot().await, before);
        sqlx::query("UPDATE organizations SET verification_status='REJECTED' WHERE id=$1")
            .bind(id(if actor == BUYER { 0x1000 } else { 0x2000 }))
            .execute(&mut f.admin)
            .await
            .unwrap();
        ok(f.deliver(actor, Some("delivery")).await, "DELIVERED").await;
        sqlx::query("UPDATE organizations SET verification_status='APPROVED' WHERE id=$1")
            .bind(id(if actor == BUYER { 0x1000 } else { 0x2000 }))
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert_eq!(
            f.deliver(actor, Some("delivery")).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(f.snapshot().await, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn private_reads_follow_current_organization_or_rejected_recorded_side() {
    for actor in [BUYER, SELLER] {
        for change in ["moved", "detached", "org_reject", "rejected"] {
            let mut f = Fixture::new().await;
            f.change(actor, change).await;
            if change == "org_reject" {
                f.change(actor, "detached").await;
            }
            let before = f.snapshot().await;
            for summary in [false, true] {
                let (status, body) = f.read(actor, summary).await;
                if change == "detached" {
                    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
                    assert!(body.get("items").is_none());
                } else {
                    assert_eq!(status, StatusCode::OK, "{body}");
                    assert_eq!(
                        body[if summary { "total_count" } else { "total" }],
                        if change == "moved" { 0 } else { 1 }
                    );
                    if !summary && change == "moved" {
                        assert_eq!(body["items"], json!([]));
                    }
                }
            }
            assert_eq!(f.snapshot().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn lifecycle_receipts_revalidate_membership_before_replay() {
    for action in ["confirm", "decline"] {
        for change in ["moved", "detached", "org_reject"] {
            let mut f = Fixture::new().await;
            sqlx::query("SELECT membership_fixture_pending($1)")
                .bind(f.trade)
                .execute(&mut f.admin)
                .await
                .unwrap();
            ok(
                f.call(action, SELLER, Some("lifecycle"), Value::Null).await,
                if action == "confirm" {
                    "CONFIRMED"
                } else {
                    "DECLINED"
                },
            )
            .await;
            f.change(SELLER, change).await;
            if change == "org_reject" {
                f.change(SELLER, "detached").await;
            }
            let before = f.snapshot().await;
            let (status, body) = f.call(action, SELLER, Some("lifecycle"), Value::Null).await;
            assert_eq!(
                status,
                if change == "org_reject" {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
                "{action} {change}: {body}"
            );
            assert_eq!(f.snapshot().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn rejected_organization_cannot_create_a_new_trade() {
    let mut f = Fixture::new().await;
    f.change(BUYER, "org_reject").await;
    let before = f.snapshot().await;
    let now = chrono::Utc::now().timestamp();
    let token = encode(&Header::default(), &json!({"sub":id(BUYER),"type":"access","iat":now,"exp":now+600,"iss":"trades-tests","aud":"trades-tests"}), &EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let response = f.router.clone().oneshot(Request::builder().method("POST").uri("/api/trades/")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(json!({"order_id":id(1),"quantity_mt":"1","expected_terms_digest":"a".repeat(64)}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn membership_move_serializes_before_settlement_authority() {
    let mut f = Fixture::new().await;
    let mut holder = PgConnection::connect(&f._database.admin_url).await.unwrap();
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
        .bind(id(0x2000))
        .execute(&mut holder)
        .await
        .unwrap();
    let before = f.snapshot().await;
    let router = f.router.clone();
    let trade = f.trade;
    let task = tokio::spawn(async move {
        send(
            &router,
            trade,
            "deliver",
            Some(SELLER),
            &[],
            delivery().to_string(),
            0,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid())")
                .fetch_one(&mut f.admin).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    sqlx::query("SELECT trades_fixture_change($1,'moved')")
        .bind(id(SELLER))
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
    let (status, body) = task.await.unwrap();
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::CONFLICT),
        "{body}"
    );
    assert!(body.get("status").is_none());
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn lifecycle_collision_recovery_revalidates_membership_and_request() {
    use std::sync::{Arc, atomic::Ordering};
    use verdaxis_read::market_lifecycle::{Delivery, LifecycleAction, lifecycle_hash, settlement_hash};
    use verdaxis_read::routes::market_write::trade_lifecycle::{ReceiptCollision, contract_router_with_collision};
    for action in ["confirm", "decline", "deliver", "pay"] {
        for change in ["current", "moved", "detached", "org_reject"] {
            for matching in [true, false] {
                let mut f = Fixture::new().await;
                if matches!(action, "confirm" | "decline") {
                    sqlx::query("SELECT membership_fixture_pending($1)").bind(f.trade).execute(&mut f.admin).await.unwrap();
                } else if action == "pay" {
                    ok(f.deliver(SELLER, None).await, "DELIVERED").await;
                }
                sqlx::query("SELECT confirm_fixture_collision_install()").execute(&mut f.admin).await.unwrap();
                let hook = Arc::new(ReceiptCollision::default());
                let pool = PgPoolOptions::new().max_connections(4).connect(&f._database.runtime_url).await.unwrap();
                let router = contract_router_with_collision(pool.clone(), KEY, hook.clone());
                let trade = f.trade;
                let body = if action == "deliver" { delivery() } else { Value::Null };
                let task = tokio::spawn(async move {
                    send(&router, trade, action, Some(SELLER), &[("idempotency-key", "collision")], body.to_string(), 0).await
                });
                tokio::time::timeout(std::time::Duration::from_secs(30), hook.entered.notified()).await.unwrap();
                assert_eq!(hook.executions.load(Ordering::SeqCst), 1);
                sqlx::query("SELECT confirm_fixture_collision_remove()").execute(&mut f.admin).await.unwrap();
                let hash = match action {
                    "confirm" => lifecycle_hash(LifecycleAction::Confirm, f.trade).unwrap(),
                    "decline" => lifecycle_hash(LifecycleAction::Decline, f.trade).unwrap(),
                    "deliver" => settlement_hash(f.trade, Some(&Delivery::parse(&delivery()).unwrap())).unwrap(),
                    _ => settlement_hash(f.trade, None).unwrap(),
                };
                let winner = json!({"stored_private_body":action});
                sqlx::query("SELECT confirm_fixture_receipt($1,$2,$3,$4,$5)")
                    .bind(f.trade).bind(format!("trade.{action}")).bind(hash).bind(matching).bind(&winner)
                    .execute(&mut f.admin).await.unwrap();
                if change != "current" { f.change(SELLER, change).await; }
                if change == "org_reject" { f.change(SELLER, "detached").await; }
                let before = f.snapshot().await;
                let receipt_before: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM market_command_results r").fetch_one(&mut f.admin).await.unwrap();
                hook.release.notify_one();
                let (status, body) = task.await.unwrap();
                let expected = if matches!(change, "moved" | "detached") { StatusCode::FORBIDDEN }
                    else if matching { StatusCode::OK } else { StatusCode::CONFLICT };
                assert_eq!(status, expected, "{action} {change} matching={matching}: {body}");
                if expected == StatusCode::OK { assert_eq!(body, winner); }
                else { assert!(body.get("stored_private_body").is_none()); assert!(body.get("freeze").is_none()); }
                assert_eq!(f.snapshot().await, before, "{action} {change}");
                let receipt_after: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM market_command_results r").fetch_one(&mut f.admin).await.unwrap();
                assert_eq!(receipt_after, receipt_before);
                assert_eq!(hook.executions.load(Ordering::SeqCst), 1);
                pool.close().await;
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn moved_actor_authority_precedes_linked_resource_consistency() {
    for action in ["confirm", "decline", "deliver", "pay"] {
        for inconsistent in ["slice", "inventory"] {
            let mut f = Fixture::new().await;
            if matches!(action, "confirm" | "decline") {
                sqlx::query("SELECT membership_fixture_pending($1)").bind(f.trade).execute(&mut f.admin).await.unwrap();
            } else if action == "pay" { ok(f.deliver(SELLER, None).await, "DELIVERED").await; }
            sqlx::query("SELECT membership_fixture_inconsistent($1,$2)").bind(f.trade).bind(inconsistent).execute(&mut f.admin).await.unwrap();
            f.change(SELLER, "moved").await;
            let before = f.snapshot().await;
            for key in [None, Some("consistency")] {
                let (status, body) = f.call(action, SELLER, key, if action == "deliver" { delivery() } else { Value::Null }).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{action} {inconsistent}: {body}");
                assert_eq!(body["detail"]["code"], "TRADE_PARTY_FORBIDDEN");
                assert_eq!(f.snapshot().await, before);
            }
        }
    }
}

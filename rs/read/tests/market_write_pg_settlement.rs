#![cfg(feature = "contract-tests")]
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::trade_lifecycle::contract_router;

const KEY: &str = "synthetic-settlement-signing-key-at-least-32";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const MEMBER: u128 = 0x1002;
const STRANGER: u128 = 0x3001;
const ADMIN: u128 = 0x4001;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn delivery() -> Value {
    json!({"final_quantity_mt":"3","final_price_per_mt":"100"})
}
fn amount(v: &Value) -> rust_decimal::Decimal {
    v.as_str().unwrap().parse().unwrap()
}
struct Fixture {
    admin: PgConnection,
    pool: PgPool,
    router: Router,
    trade: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let a = std::env::var("RUST_SETTLEMENT_ADMIN_DATABASE_URL").expect("disposable admin URL");
        let r = std::env::var("RUST_SETTLEMENT_DATABASE_URL").expect("disposable runtime URL");
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
        let router = contract_router(pool.clone(), KEY, true);
        Self {
            admin,
            pool,
            router,
            trade,
        }
    }
    async fn sql(&mut self, sql: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn change(&mut self, actor: u128, change: &str) {
        sqlx::query("SELECT CASE WHEN $2='org_reject' THEN settlement_fixture_reject_org($1) ELSE trades_fixture_change($1,$2) END")
            .bind(id(actor))
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
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
    async fn assisted(&mut self) {
        self.sql("SELECT confirm_fixture_assisted(); SELECT confirm_fixture_pending('BUYER',false); SELECT settlement_fixture_confirmed()").await;
        self.trade = sqlx::query_scalar("SELECT id FROM trades")
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
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
async fn detached_party_delivers_and_pays_its_preexisting_trade() {
    let mut f = Fixture::new().await;
    f.change(SELLER, "detached").await;
    let original = f.snapshot().await;
    let delivered = ok(f.deliver(SELLER, Some("delivery")).await, "DELIVERED").await;
    assert_eq!(
        amount(&delivered["final_total_usd"]),
        "300".parse().unwrap()
    );
    assert_eq!(
        amount(&delivered["commission_amount_usd"]),
        "6".parse().unwrap()
    );
    ok(f.pay(SELLER, Some("payment")).await, "PAID").await;
    let after = f.snapshot().await;
    for field in ["orders", "inventory", "referrals", "benchmarks", "targets"] {
        assert_eq!(after[field], original[field], "{field}");
    }
    assert_eq!(after["audit"], 2);
    assert_eq!(after["outbox"], 2);
    assert_eq!(after["receipts"], 2);
    let receipts: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT effective_organization_id,support_context_id FROM market_command_results",
    )
    .fetch_all(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(receipts, vec![(id(0x2000), None); 2]);
    assert_eq!(
        ok(f.deliver(SELLER, Some("delivery")).await, "DELIVERED").await,
        delivered
    );
    assert_eq!(f.snapshot().await, after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn deliver_accepts_either_recorded_party_and_rejects_strangers() {
    for actor in [BUYER, SELLER] {
        let mut f = Fixture::new().await;
        for denied in [MEMBER, STRANGER, ADMIN] {
            let before = f.snapshot().await;
            let (s, b) = f.deliver(denied, None).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "actor={denied}: {b}");
            assert_eq!(f.snapshot().await, before);
        }
        ok(f.deliver(actor, None).await, "DELIVERED").await;
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn pay_requires_the_recorded_seller_and_supplier_role() {
    let mut f = Fixture::new().await;
    ok(f.deliver(BUYER, None).await, "DELIVERED").await;
    for actor in [BUYER, MEMBER, STRANGER, ADMIN] {
        let before = f.snapshot().await;
        let (s, b) = f.pay(actor, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
        assert_eq!(f.snapshot().await, before);
    }
    f.sql("UPDATE users SET role='BUYER' WHERE id='00000000-0000-0000-0000-000000002001'")
        .await;
    assert_eq!(f.pay(SELLER, None).await.0, StatusCode::FORBIDDEN);
    f.sql("UPDATE users SET role='SUPPLIER' WHERE id='00000000-0000-0000-0000-000000002001'")
        .await;
    ok(f.pay(SELLER, None).await, "PAID").await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn keyed_and_unkeyed_cleanup_after_rejection_unlink_and_move() {
    for change in [
        "rejected",
        "detached",
        "moved",
        "org_reject",
        "kyc",
        "email",
        "password",
    ] {
        for key in [None, Some("cleanup")] {
            let mut f = Fixture::new().await;
            f.change(SELLER, change).await;
            ok(f.deliver(SELLER, key).await, "DELIVERED").await;
            ok(f.pay(SELLER, key).await, "PAID").await;
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn detached_party_replays_a_rust_receipt() {
    for change in ["rejected", "detached", "moved", "org_reject"] {
        let mut f = Fixture::new().await;
        let delivered = ok(f.deliver(SELLER, Some("settle")).await, "DELIVERED").await;
        let paid = ok(f.pay(SELLER, Some("settle")).await, "PAID").await;
        f.change(SELLER, change).await;
        let before = f.snapshot().await;
        assert_eq!(
            ok(f.deliver(SELLER, Some(" settle ")).await, "DELIVERED").await,
            delivered
        );
        assert_eq!(ok(f.pay(SELLER, Some("settle")).await, "PAID").await, paid);
        assert_eq!(f.snapshot().await, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn delivery_deviation_boundary_is_inclusive() {
    for (price, status) in [
        ("90", 200),
        ("110", 200),
        ("90.01", 200),
        ("109.99", 200),
        ("89.99", 400),
        ("110.01", 400),
    ] {
        let mut f = Fixture::new().await;
        let before = f.snapshot().await;
        let (s, b) = f
            .call(
                "deliver",
                BUYER,
                None,
                json!({"final_quantity_mt":"3","final_price_per_mt":price}),
            )
            .await;
        assert_eq!(s.as_u16(), status, "{price}: {b}");
        if status != 200 {
            assert_eq!(f.snapshot().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn delivery_quantity_cannot_exceed_the_trade() {
    let mut f = Fixture::new().await;
    let before = f.snapshot().await;
    let (s, b) = f
        .call(
            "deliver",
            BUYER,
            None,
            json!({"final_quantity_mt":"3.01","final_price_per_mt":"100"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(f.snapshot().await, before);
    ok(f.deliver(BUYER, None).await, "DELIVERED").await;
    assert_eq!(f.deliver(BUYER, None).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn settlement_rounds_once_half_up() {
    for (fee, rate, total, commission) in [
        (Some("1.50"), "0", "3.02", "0.05"),
        (None, "16.550", "3.02", "0.50"),
        (Some("0"), "0", "3.02", "0"),
    ] {
        let mut f = Fixture::new().await;
        sqlx::query("SELECT settlement_fixture_confirmed(0.03,100.50,$1,$2)")
            .bind(fee.map(|s| s.parse::<rust_decimal::Decimal>().unwrap()))
            .bind(rate.parse::<rust_decimal::Decimal>().unwrap())
            .execute(&mut f.admin)
            .await
            .unwrap();
        let b = ok(
            f.call(
                "deliver",
                SELLER,
                None,
                json!({"final_quantity_mt":"0.03","final_price_per_mt":"100.50"}),
            )
            .await,
            "DELIVERED",
        )
        .await;
        assert_eq!(amount(&b["final_total_usd"]), total.parse().unwrap());
        assert_eq!(
            amount(&b["commission_amount_usd"]),
            commission.parse().unwrap()
        );
        let commissions: i64 = sqlx::query_scalar("SELECT count(*) FROM commissions")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(commissions, 0);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn request_and_authentication_precedence_is_explicit() {
    let mut f = Fixture::new().await;
    let before = f.snapshot().await;
    for (actor, action, body, headers, expected) in [
        (None, "deliver", "{", vec![], 401),
        (Some(BUYER), "pay", "{", vec![], 403),
        (Some(BUYER), "deliver", "{", vec![], 422),
        (
            None,
            "deliver",
            "{",
            vec![("x-verdaxis-market-support-context", "not-a-context")],
            403,
        ),
        (
            Some(BUYER),
            "deliver",
            "{",
            vec![("x-verdaxis-market-support-context", " ")],
            422,
        ),
    ] {
        let (s, b) = send(&f.router, f.trade, action, actor, &headers, body.into(), 0).await;
        assert_eq!(s.as_u16(), expected, "{b}");
        assert_eq!(f.snapshot().await, before);
    }
    for body in [
        "{}",
        "null",
        r#"{"final_quantity_mt":null,"final_price_per_mt":100}"#,
        r#"{"final_quantity_mt":1e0,"final_price_per_mt":100}"#,
    ] {
        assert_eq!(
            send(
                &f.router,
                f.trade,
                "deliver",
                Some(BUYER),
                &[],
                body.into(),
                0
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        send(
            &f.router,
            f.trade,
            "deliver",
            Some(BUYER),
            &[],
            " ".repeat(65537),
            0
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    for key in [" ".to_owned(), "x".repeat(256)] {
        assert_eq!(
            f.deliver(BUYER, Some(&key)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn password_cutoff_always_applies_and_only_rejection_skips_revocation() {
    for (state, cutoff, expected) in [
        ("REJECTED", "password_changed_at", 401),
        ("APPROVED", "authentication_revoked_at", 401),
        ("PENDING", "authentication_revoked_at", 401),
        ("REJECTED", "authentication_revoked_at", 200),
    ] {
        let mut f = Fixture::new().await;
        f.sql(&format!("UPDATE users SET status='{state}',{cutoff}=now()-interval '30 seconds' WHERE id='00000000-0000-0000-0000-000000002001'")).await;
        let before = f.snapshot().await;
        let (s, b) = send(
            &f.router,
            f.trade,
            "deliver",
            Some(SELLER),
            &[],
            delivery().to_string(),
            -60,
        )
        .await;
        assert_eq!(s.as_u16(), expected, "{state} {cutoff}: {b}");
        if expected != 200 {
            assert_eq!(f.snapshot().await, before);
        }
    }
    let f = Fixture::new().await;
    assert_eq!(
        send(
            &f.router,
            f.trade,
            "deliver",
            Some(SELLER),
            &[],
            delivery().to_string(),
            -2000
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn stranger_and_cross_trade_actor_are_refused() {
    let mut f = Fixture::new().await;
    ok(f.deliver(SELLER, Some("same")).await, "DELIVERED").await;
    let before = f.snapshot().await;
    assert_eq!(
        f.deliver(STRANGER, Some("same")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.call(
            "deliver",
            SELLER,
            Some("same"),
            json!({"final_quantity_mt":"2","final_price_per_mt":"100"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let other: Uuid = sqlx::query_scalar("SELECT settlement_fixture_clone()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let before_other = f.snapshot().await;
    let (s, b) = send(
        &f.router,
        other,
        "deliver",
        Some(SELLER),
        &[("idempotency-key", "same")],
        delivery().to_string(),
        0,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(b["detail"]["code"], "RECEIPT_RESOURCE_MISMATCH");
    assert_eq!(f.snapshot().await, before_other);
    assert_eq!(
        send(
            &f.router,
            Uuid::new_v4(),
            "deliver",
            Some(SELLER),
            &[("idempotency-key", "same")],
            delivery().to_string(),
            0
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(before["audit"], before_other["audit"]);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn old_python_receipt_replays_after_detachment_in_rust() {
    let mut f = Fixture::new().await;
    let body = json!({"status":"DELIVERED","original_response":true});
    // The persisted hash is the Python canonical Decimal/UUID payload.
    let payload = json!({"operation":"trade.deliver","trade_id":f.trade,"final_quantity_mt":"3","final_price_per_mt":"100"});
    use sha2::{Digest, Sha256};
    let mut entries = std::collections::BTreeMap::new();
    for (k, v) in payload.as_object().unwrap() {
        entries.insert(k, v);
    }
    let hash = Sha256::digest(serde_json::to_vec(&entries).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    sqlx::query("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body) VALUES($1,$2,$3,'trade.deliver','old',$4,'trade',$5,200,$6)").bind(Uuid::new_v4()).bind(id(SELLER)).bind(id(0x2000)).bind(hash).bind(f.trade).bind(&body).execute(&mut f.admin).await.unwrap();
    f.change(SELLER, "detached").await;
    let before = f.snapshot().await;
    assert_eq!(
        ok(f.deliver(SELLER, Some("old")).await, "DELIVERED").await,
        body
    );
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn assisted_customers_settle_but_staff_and_former_members_do_not() {
    let mut f = Fixture::new().await;
    f.assisted().await;
    assert_eq!(f.deliver(ADMIN, None).await.0, StatusCode::FORBIDDEN);
    ok(f.deliver(SELLER, Some("assisted")).await, "DELIVERED").await;
    f.change(SELLER, "detached").await;
    let before = f.snapshot().await;
    assert_eq!(
        f.deliver(SELLER, Some("assisted")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.pay(SELLER, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(f.snapshot().await, before);
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    ok(f.pay(SELLER, None).await, "PAID").await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn settlement_effects_have_exact_scope_and_transition_time() {
    for actor in [BUYER, SELLER] {
        let mut f = Fixture::new().await;
        f.sql("UPDATE users SET status='REJECTED',email_verified=false WHERE id='00000000-0000-0000-0000-000000001002'; SELECT confirm_fixture_mirrors()").await;
        let before = f.snapshot().await;
        let delivered = ok(f.deliver(actor, Some("delivery")).await, "DELIVERED").await;
        let paid = ok(f.pay(SELLER, Some("pay")).await, "PAID").await;
        let after = f.snapshot().await;
        for field in ["orders", "inventory", "referrals", "benchmarks", "targets"] {
            assert_eq!(before[field], after[field], "{field}");
        }
        let rows:Vec<(String,Value,Value)>=sqlx::query_as("SELECT event_type,payload,participant_org_ids FROM market_event_outbox ORDER BY created_at,id").fetch_all(&mut f.admin).await.unwrap();
        assert_eq!(rows.len(), 2);
        for (kind, payload, orgs) in rows {
            assert_eq!(orgs, json!([id(0x1000), id(0x2000)]));
            assert_eq!(payload["schema_version"], 1);
            assert_eq!(payload["source_kind"], "CONFIRMED_TRADE");
            assert_eq!(payload["demo_status"], "REAL_ONLY");
            assert_eq!(payload["scope"], "DELIVERY_POINT");
            assert!(payload.get("_public_market_invalidation").is_none());
            assert!(payload.get("commission_amount_usd").is_none());
            let timestamp = payload["observed_at"]
                .as_str()
                .unwrap()
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap();
            if kind == "trade_delivered" {
                assert_eq!(
                    timestamp,
                    delivered["delivered_at"]
                        .as_str()
                        .unwrap()
                        .parse::<chrono::DateTime<chrono::Utc>>()
                        .unwrap()
                );
                assert_eq!(amount(&payload["final_total"]), "300".parse().unwrap());
                assert!(payload.get("quantity").is_none());
            } else {
                assert_eq!(kind, "trade_paid");
                assert_eq!(
                    timestamp,
                    paid["paid_at"]
                        .as_str()
                        .unwrap()
                        .parse::<chrono::DateTime<chrono::Utc>>()
                        .unwrap()
                );
                assert_eq!(amount(&payload["quantity"]), "3".parse().unwrap());
                assert!(payload.get("final_total").is_none());
            }
        }
        let notices: Vec<(Uuid, String, String, Value)> = sqlx::query_as(
            "SELECT recipient_id,title,message,data FROM notifications ORDER BY recipient_id,title",
        )
        .fetch_all(&mut f.admin)
        .await
        .unwrap();
        let delivery_org = if actor == BUYER {
            id(0x2000)
        } else {
            id(0x1000)
        };
        let delivery_recipients: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
                .bind(delivery_org)
                .fetch_all(&mut f.admin)
                .await
                .unwrap();
        let actual: Vec<_> = notices
            .iter()
            .filter(|n| n.1 == "Trade Delivered")
            .map(|n| n.0)
            .collect();
        assert_eq!(actual, delivery_recipients);
        for (_, title, message, data) in &notices {
            assert_eq!(*data, json!({"trade_id":f.trade}));
            assert_eq!(
                message,
                if title == "Trade Delivered" {
                    "Trade for 3 MT has been marked as delivered."
                } else {
                    "Payment for trade of 3 MT has been confirmed."
                }
            );
        }
        let audits: Vec<(String, Value)> =
            sqlx::query_as("SELECT action,changes FROM audit_logs ORDER BY timestamp,id")
                .fetch_all(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(audits.len(), 2);
        assert_eq!(audits[0].0, "trade.delivered");
        assert_eq!(
            audits[0].1,
            json!({"status":"DELIVERED","final_quantity_mt":"3","final_price_per_mt":"100","final_total_usd":"300"})
        );
        assert_eq!(audits[1], ("trade.paid".into(), json!({"status":"PAID"})));
        if actor == BUYER {
            assert!(delivered["commission_amount_usd"].is_null());
        } else {
            assert!(!delivered["commission_amount_usd"].is_null());
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn no_current_recipients_produces_no_notifications() {
    let mut f = Fixture::new().await;
    f.sql("UPDATE users SET organization_id=NULL WHERE organization_id='00000000-0000-0000-0000-000000001000'").await;
    ok(f.deliver(SELLER, None).await, "DELIVERED").await;
    ok(f.pay(SELLER, None).await, "PAID").await;
    assert_eq!(f.snapshot().await["notifications"], 0);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn failed_effect_rolls_back_final_fields_and_receipt() {
    let mut f = Fixture::new().await;
    let before = f.snapshot().await;
    f.sql("CREATE TRIGGER settlement_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION settlement_fixture_reject_audit()").await;
    let result = f.deliver(SELLER, Some("rollback")).await;
    f.sql("DROP TRIGGER settlement_audit ON audit_logs").await;
    assert!(!result.0.is_success(), "{}", result.1);
    assert_eq!(f.snapshot().await, before);
}

async fn wait_for_lock(conn: &mut PgConnection) {
    tokio::time::timeout(std::time::Duration::from_secs(20),async {
        loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid())").fetch_one(&mut *conn).await.unwrap();if waiting {break;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
    }).await.expect("observed settlement lock wait");
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn deliver_and_pay_serialize_in_both_orders() {
    for first in ["deliver", "pay"] {
        let mut f = Fixture::new().await;
        let mut holder =
            PgConnection::connect(&std::env::var("RUST_SETTLEMENT_ADMIN_DATABASE_URL").unwrap())
                .await
                .unwrap();
        sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
        sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE")
            .bind(f.trade)
            .execute(&mut holder)
            .await
            .unwrap();
        let router = f.router.clone();
        let trade = f.trade;
        let first_task = tokio::spawn(async move {
            send(
                &router,
                trade,
                first,
                Some(SELLER),
                &[],
                delivery().to_string(),
                0,
            )
            .await
        });
        wait_for_lock(&mut f.admin).await;
        let router = f.router.clone();
        let second = if first == "deliver" { "pay" } else { "deliver" };
        let second_task = tokio::spawn(async move {
            send(
                &router,
                trade,
                second,
                Some(SELLER),
                &[],
                delivery().to_string(),
                0,
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
        let a = first_task.await.unwrap();
        let b = second_task.await.unwrap();
        if first == "deliver" {
            ok(a, "DELIVERED").await;
            ok(b, "PAID").await;
            assert_eq!(f.snapshot().await["audit"], 2);
        } else {
            assert_eq!(a.0, StatusCode::BAD_REQUEST, "{}", a.1);
            assert_eq!(a.1["detail"]["code"], "TRADE_NOT_DELIVERED");
            ok(b, "DELIVERED").await;
            let after = f.snapshot().await;
            assert_eq!(after["audit"], 1);
            assert!(after["trades"][0]["paid_at"].is_null());
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn concurrent_identical_requests_commit_one_receipt_and_effect_set() {
    let mut f = Fixture::new().await;
    for action in ["deliver", "pay"] {
        let (a, b) = tokio::join!(
            f.call(action, SELLER, Some("one"), delivery()),
            f.call(action, SELLER, Some("one"), delivery())
        );
        for response in [a, b] {
            if response.0 != StatusCode::OK {
                assert!(
                    matches!(
                        response.1["detail"]["code"].as_str(),
                        Some("MARKET_BUSY" | "IDEMPOTENCY_BUSY" | "TRANSACTION_RETRY_EXHAUSTED")
                    ),
                    "{}",
                    response.1
                );
            }
        }
        let status = if action == "deliver" {
            "DELIVERED"
        } else {
            "PAID"
        };
        let body = ok(
            f.call(action, SELLER, Some("one"), delivery()).await,
            status,
        )
        .await;
        assert_eq!(
            ok(
                f.call(action, SELLER, Some("one"), delivery()).await,
                status
            )
            .await,
            body
        );
    }
    let after = f.snapshot().await;
    assert_eq!(after["receipts"], 2);
    assert_eq!(after["audit"], 2);
    assert_eq!(after["outbox"], 2);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn retryable_abort_rebuilds_once_without_duplicate_effects() {
    let mut f = Fixture::new().await;
    f.sql("SELECT confirm_fixture_abort_install()").await;
    let result = f.deliver(SELLER, Some("retry")).await;
    f.sql("SELECT confirm_fixture_abort_remove()").await;
    ok(result, "DELIVERED").await;
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM confirm_fixture_attempts")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(attempts, 2);
    let after = f.snapshot().await;
    assert_eq!(after["receipts"], 1);
    assert_eq!(after["audit"], 1);
    assert_eq!(after["outbox"], 1);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn receipt_collision_revalidates_recorded_side_before_replay() {
    use std::sync::Arc;
    use verdaxis_read::{
        market_lifecycle::{Delivery, settlement_hash},
        routes::market_write::trade_lifecycle::{ReceiptCollision, contract_router_with_collision},
    };
    for (assisted, change, matching, expected) in [
        (false, false, true, 200),
        (false, false, false, 409),
        (true, true, true, 403),
    ] {
        let mut f = Fixture::new().await;
        if assisted {
            f.assisted().await;
        }
        f.sql("SELECT confirm_fixture_collision_install()").await;
        let hook = Arc::new(ReceiptCollision::default());
        let router = contract_router_with_collision(f.pool.clone(), KEY, hook.clone());
        let trade = f.trade;
        let before = f.snapshot().await;
        let task = tokio::spawn(async move {
            send(
                &router,
                trade,
                "deliver",
                Some(SELLER),
                &[("idempotency-key", "collision")],
                delivery().to_string(),
                0,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), hook.entered.notified())
            .await
            .unwrap();
        f.sql("SELECT confirm_fixture_collision_remove()").await;
        let hash = settlement_hash(f.trade, Some(&Delivery::parse(&delivery()).unwrap())).unwrap();
        sqlx::query("SELECT confirm_fixture_receipt($1,'trade.deliver',$2,$3,$4)")
            .bind(f.trade)
            .bind(hash)
            .bind(matching)
            .bind(json!({"winner":true}))
            .execute(&mut f.admin)
            .await
            .unwrap();
        if change {
            f.change(SELLER, "detached").await;
        }
        hook.release.notify_one();
        let (s, b) = task.await.unwrap();
        assert_eq!(s.as_u16(), expected, "{b}");
        if expected == 200 {
            assert_eq!(b, json!({"winner":true}));
        } else {
            assert!(b.get("winner").is_none());
        }
        assert_eq!(hook.executions.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut after = f.snapshot().await;
        assert_eq!(after["receipts"], 1);
        after["receipts"] = before["receipts"].clone();
        assert_eq!(after, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn receipt_context_mismatch_returns_no_stored_body() {
    for mismatch in ["organization", "support", "resource_type"] {
        let mut f = Fixture::new().await;
        let hash = verdaxis_read::market_lifecycle::settlement_hash(
            f.trade,
            Some(&verdaxis_read::market_lifecycle::Delivery::parse(&delivery()).unwrap()),
        )
        .unwrap();
        let context = if mismatch == "support" {
            Some(
                sqlx::query_scalar::<_, Uuid>("SELECT trades_fixture_support(false)")
                    .fetch_one(&mut f.admin)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        sqlx::query("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,support_context_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body) VALUES($1,$2,$3,$4,'trade.deliver','context',$5,$6,$7,200,jsonb_build_object('private_body',true))")
            .bind(Uuid::new_v4()).bind(id(SELLER)).bind(if mismatch=="organization"{id(0x1000)}else{id(0x2000)}).bind(context).bind(hash).bind(if mismatch=="resource_type"{"order"}else{"trade"}).bind(f.trade).execute(&mut f.admin).await.unwrap();
        let before = f.snapshot().await;
        let (s, b) = f.deliver(SELLER, Some("context")).await;
        assert_eq!(s, StatusCode::CONFLICT, "{b}");
        assert!(b.get("private_body").is_none());
        assert_eq!(f.snapshot().await, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn receipt_lock_contention_is_bounded_and_has_no_effect() {
    let mut f = Fixture::new().await;
    let mut holder =
        PgConnection::connect(&std::env::var("RUST_SETTLEMENT_ADMIN_DATABASE_URL").unwrap())
            .await
            .unwrap();
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT settlement_fixture_hold_key($1)")
        .bind(id(SELLER))
        .execute(&mut holder)
        .await
        .unwrap();
    let before = f.snapshot().await;
    let (s, b) = f.deliver(SELLER, Some("held")).await;
    sqlx::query("ROLLBACK").execute(&mut holder).await.unwrap();
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{b}");
    assert_eq!(b["detail"]["code"], "IDEMPOTENCY_BUSY");
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn recorded_party_history_and_counts_survive_detachment() {
    let mut f = Fixture::new().await;
    f.change(SELLER, "detached").await;
    ok(f.deliver(SELLER, None).await, "DELIVERED").await;
    ok(f.pay(SELLER, None).await, "PAID").await;
    let router =
        verdaxis_read::routes::market_write::trades::contract_router(f.pool.clone(), KEY, true);
    let now = chrono::Utc::now().timestamp();
    let token=encode(&Header::default(),&json!({"sub":id(SELLER),"type":"access","iat":now,"exp":now+600,"iss":"trades-tests","aud":"trades-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    for path in ["/api/trades/my", "/api/trades/summary"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
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
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        if path.ends_with("/my") {
            assert_eq!(body["total"], 1);
            assert_eq!(body["items"][0]["status"], "PAID");
            assert_eq!(body["items"][0]["seller_id"], id(0x2000).to_string());
        } else {
            assert_eq!(body["total_count"], 1);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn legacy_null_principals_do_not_grant_organization_authority() {
    for actor in [BUYER, SELLER] {
        let mut f = Fixture::new().await;
        sqlx::query("SELECT settlement_fixture_null_party($1)")
            .bind(actor == SELLER)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert_eq!(f.deliver(actor, None).await.0, StatusCode::FORBIDDEN);
        assert_eq!(f.snapshot().await, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn distinct_actor_scopes_and_reverse_resource_order_remain_separate() {
    let mut f = Fixture::new().await;
    let first = f.trade;
    let second: Uuid = sqlx::query_scalar("SELECT settlement_fixture_clone()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    for (trade, actor) in [(first, SELLER), (second, BUYER)] {
        ok(
            send(
                &f.router,
                trade,
                "deliver",
                Some(actor),
                &[("idempotency-key", "shared")],
                delivery().to_string(),
                0,
            )
            .await,
            "DELIVERED",
        )
        .await;
    }
    let before = f.snapshot().await;
    for (trade, actor) in [(second, SELLER), (first, BUYER)] {
        let (s, b) = send(
            &f.router,
            trade,
            "deliver",
            Some(actor),
            &[("idempotency-key", "shared")],
            delivery().to_string(),
            0,
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "{b}");
        assert_eq!(b["detail"]["code"], "RECEIPT_RESOURCE_MISMATCH");
    }
    assert_eq!(f.snapshot().await, before);
    assert_eq!(before["receipts"], 2);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn payment_failure_rolls_back_paid_timestamp_and_effects() {
    let mut f = Fixture::new().await;
    ok(f.deliver(SELLER, None).await, "DELIVERED").await;
    let before = f.snapshot().await;
    f.sql("CREATE TRIGGER settlement_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION settlement_fixture_reject_audit()").await;
    let result = f.pay(SELLER, Some("rollback")).await;
    f.sql("DROP TRIGGER settlement_audit ON audit_logs").await;
    assert!(!result.0.is_success(), "{}", result.1);
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn legacy_fee_is_visible_to_the_recorded_buyer() {
    let mut f = Fixture::new().await;
    f.sql("SELECT settlement_fixture_confirmed(3,100,NULL,1.125)")
        .await;
    f.change(BUYER, "moved").await;
    let body = ok(f.deliver(BUYER, None).await, "DELIVERED").await;
    assert_eq!(
        amount(&body["commission_amount_usd"]),
        "3.38".parse().unwrap()
    );
    assert_eq!(body["buyer_id"], id(0x1000).to_string());
    assert_eq!(body["seller_id"], id(0x2000).to_string());
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn delivery_domain_refusals_leave_no_partial_state() {
    let mut f = Fixture::new().await;
    let before = f.snapshot().await;
    for (field, invalid) in [
        (
            "final_quantity_mt",
            vec![
                "0",
                "-1",
                "100000.01",
                "1.001",
                "1.000",
                "+1",
                " 1",
                "1 ",
                "-0",
                "NaN",
                "Infinity",
            ],
        ),
        (
            "final_price_per_mt",
            vec![
                "0",
                "-1",
                "1000000.01",
                "100.001",
                "100.000",
                "+100",
                " 100",
                "-0",
                "NaN",
                "Infinity",
            ],
        ),
    ] {
        for invalid in invalid {
            let mut body = delivery();
            body[field] = json!(invalid);
            let (s, b) = f.call("deliver", SELLER, Some("invalid"), body).await;
            assert_eq!(
                s,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field} {invalid}: {b}"
            );
            assert_eq!(f.snapshot().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn both_recorded_sides_keep_cleanup_without_fresh_admission() {
    for actor in [BUYER, SELLER] {
        for change in [
            "rejected",
            "pending",
            "detached",
            "moved",
            "org_reject",
            "email",
            "password",
            "kyc",
        ] {
            let mut f = Fixture::new().await;
            f.change(actor, change).await;
            ok(f.deliver(actor, None).await, "DELIVERED").await;
        }
    }
    let mut f = Fixture::new().await;
    f.change(SELLER, "rejected").await;
    let before = f.snapshot().await;
    assert_eq!(
        f.call("confirm", SELLER, None, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    let router =
        verdaxis_read::routes::market_write::trades::contract_router(f.pool.clone(), KEY, true);
    let now = chrono::Utc::now().timestamp();
    let token=encode(&Header::default(),&json!({"sub":id(SELLER),"type":"access","iat":now,"exp":now+600,"iss":"trades-tests","aud":"trades-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/trades/")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(json!({"order_id":id(1),"quantity_mt":"1","expected_terms_digest":"a".repeat(64)}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn unkeyed_competing_settlement_has_one_transition() {
    let mut f = Fixture::new().await;
    for action in ["deliver", "pay"] {
        let (a, b) = tokio::join!(
            f.call(action, SELLER, None, delivery()),
            f.call(action, SELLER, None, delivery())
        );
        let successes = usize::from(a.0 == StatusCode::OK) + usize::from(b.0 == StatusCode::OK);
        assert_eq!(successes, 1, "{a:?} {b:?}");
        for response in [a, b] {
            if response.0 != StatusCode::OK {
                assert!(
                    response.0 == StatusCode::BAD_REQUEST
                        || matches!(
                            response.1["detail"]["code"].as_str(),
                            Some("MARKET_BUSY" | "TRANSACTION_RETRY_EXHAUSTED")
                        ),
                    "{}",
                    response.1
                );
            }
        }
    }
    let after = f.snapshot().await;
    assert_eq!(after["receipts"], 0);
    assert_eq!(after["audit"], 2);
    assert_eq!(after["outbox"], 2);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn no_linked_order_settles_with_bounded_audit_identity() {
    let mut f = Fixture::new().await;
    f.sql("SELECT confirm_fixture_no_order()").await;
    let request_id = "r".repeat(1000);
    let (s, b) = send(
        &f.router,
        f.trade,
        "deliver",
        Some(SELLER),
        &[
            ("x-request-id", &request_id),
            ("x-forwarded-for", "198.51.100.9"),
        ],
        delivery().to_string(),
        0,
    )
    .await;
    ok((s, b), "DELIVERED").await;
    let (request_id, ip): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT request_id,ip_address FROM audit_logs")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert!(request_id.is_some_and(|s| !s.is_empty() && s.len() <= 128));
    assert_ne!(ip.as_deref(), Some("198.51.100.9"));
    ok(f.pay(SELLER, None).await, "PAID").await;
}

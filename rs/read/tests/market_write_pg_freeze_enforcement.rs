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
use verdaxis_read::routes::market_write::{freeze, orders, trade_lifecycle, trades};

const KEY: &str = "synthetic-lifecycle-signing-key-at-least-32";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const STRANGER: u128 = 0x3001;
const ADMIN: u128 = 0x4001;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn delivery() -> Value {
    json!({"final_quantity_mt":"3","final_price_per_mt":"100"})
}
fn router(pool: PgPool) -> Router {
    trade_lifecycle::contract_router(pool.clone(), KEY, true)
        .merge(trades::contract_router(pool.clone(), KEY, true))
        .merge(freeze::contract_router(pool.clone(), KEY))
        .merge(orders::contract_router(pool, KEY))
}
struct Fixture {
    admin: PgConnection,
    pool: PgPool,
    router: Router,
    trade: Uuid,
    admin_url: String,
    runtime_url: String,
}
impl Fixture {
    async fn new(state: &str) -> Self {
        let a = std::env::var("RUST_FREEZE_ENFORCEMENT_ADMIN_DATABASE_URL")
            .expect("disposable admin URL");
        let r =
            std::env::var("RUST_FREEZE_ENFORCEMENT_DATABASE_URL").expect("disposable runtime URL");
        let au = url::Url::parse(&a).unwrap();
        let ru = url::Url::parse(&r).unwrap();
        assert_eq!(au.host_str(), Some("127.0.0.1"));
        assert!(au.port().is_some());
        assert!(au.path().ends_with("_test"));
        assert_eq!(
            (au.host_str(), au.port(), au.path()),
            (ru.host_str(), ru.port(), ru.path())
        );
        assert_ne!(au.username(), ru.username());
        let mut admin = PgConnection::connect(&a).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT version_num FROM alembic_version")
                .fetch_one(&mut admin)
                .await
                .unwrap(),
            "p89_20261022_trade_freeze"
        );
        for sql in [
            include_str!("fixtures/phase9_trades.sql"),
            include_str!("fixtures/phase9_confirm.sql"),
            include_str!("fixtures/phase9_freeze_enforcement.sql"),
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
        let trade = sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql("SELECT confirm_fixture_referrals(); SELECT confirm_fixture_mirrors();")
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&r)
            .await
            .unwrap();
        let router = router(pool.clone());
        let f = Self {
            admin,
            pool,
            router,
            trade,
            admin_url: a,
            runtime_url: r,
        };
        if state != "PENDING_CONFIRMATION" {
            assert_eq!(
                f.action("confirm", SELLER, None, Value::Null).await.0,
                StatusCode::OK
            );
        }
        if state == "DELIVERED" {
            assert_eq!(
                f.action("deliver", SELLER, None, delivery()).await.0,
                StatusCode::OK
            );
        }
        f
    }
    async fn sql(&mut self, sql: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('domain',confirm_fixture_snapshot(),'freezes',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM trade_freezes f),'audits',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_logs a),'notifications',(SELECT jsonb_agg(to_jsonb(n) ORDER BY id) FROM notifications n),'outbox',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM market_event_outbox o),'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM market_command_results r))").fetch_one(&mut self.admin).await.unwrap()
    }
    async fn action(
        &self,
        action: &str,
        actor: u128,
        key: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        send_action(&self.router, self.trade, action, actor, key, body).await
    }
    async fn freeze(&self) -> Value {
        let (s, b) = self
            .action(
                "freeze",
                ADMIN,
                None,
                json!({"reason_code":"COMPLIANCE_REVIEW","note":"private administrative review"}),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        b
    }
    async fn clear(&self) {
        let (s, b) = self
            .action("unfreeze", ADMIN, None, json!({"note":"review complete"}))
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
    }
    async fn read(&self, actor: u128, query: &str) -> Value {
        let (s, b) = send(
            &self.router,
            "GET",
            &format!("/api/trades/my{query}"),
            actor,
            None,
            Value::Null,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        b
    }
}
async fn send(
    router: &Router,
    method: &str,
    path: &str,
    actor: u128,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let now = chrono::Utc::now().timestamp();
    // Each component verifies its own issuer and audience on the same synthetic token.
    let issuer = if path.starts_with("/api/admin/trades") {
        "freeze-tests"
    } else if path.starts_with("/api/orderbook") {
        "cancel-tests"
    } else if path.ends_with("/my")
        || path.contains("/my?")
        || path.ends_with("/summary")
        || path == "/api/trades/"
    {
        "trades-tests"
    } else {
        "lifecycle-tests"
    };
    let token = encode(
        &Header::default(),
        &json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":issuer,"aud":issuer}),
        &EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap();
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("x-request-id", "freeze-enforcement-request");
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
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
async fn send_action(
    router: &Router,
    trade: Uuid,
    action: &str,
    actor: u128,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let admin = matches!(action, "freeze" | "unfreeze");
    send(
        router,
        if admin || action == "pay" {
            "POST"
        } else {
            "PUT"
        },
        &format!(
            "/api/{}trades/{trade}/{action}",
            if admin { "admin/" } else { "" }
        ),
        actor,
        key,
        body,
    )
    .await
}
fn assert_clear(body: &Value) {
    assert_eq!(body["is_frozen"], false, "{body}");
    assert_eq!(body["freeze"], Value::Null);
}
fn assert_active(body: &Value, frozen: &Value) {
    assert_eq!(body["is_frozen"], true, "{body}");
    assert_eq!(body["freeze"].as_object().unwrap().len(), 2);
    assert_eq!(body["freeze"]["reason_code"], frozen["reason_code"]);
    let instant = |v: &Value| chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap()).unwrap();
    assert_eq!(
        instant(&body["freeze"]["set_at"]),
        instant(&frozen["frozen_at"])
    );
    assert!(!body.to_string().contains("private administrative review"));
    assert!(body.get("frozen_by_user_id").is_none());
}
fn assert_refusal(result: &(StatusCode, Value), status: StatusCode, code: &str) {
    assert_eq!(result.0, status, "{}", result.1);
    assert_eq!(result.1["detail"]["code"], code);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn frozen_trade_response_exposes_active_freeze() {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    let frozen = f.freeze().await;
    let before = f.snapshot().await;
    for actor in [BUYER, SELLER] {
        let b = f.read(actor, "").await;
        assert_eq!(b["total"], 1);
        assert_active(&b["items"][0], &frozen);
    }
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_blocks_the_transition_each_state_can_attempt() {
    for (state, action, body) in [
        ("PENDING_CONFIRMATION", "confirm", Value::Null),
        ("PENDING_CONFIRMATION", "decline", Value::Null),
        ("CONFIRMED", "deliver", delivery()),
        ("DELIVERED", "pay", Value::Null),
    ] {
        for key in [None, Some("frozen-command")] {
            let mut f = Fixture::new(state).await;
            f.freeze().await;
            let before = f.snapshot().await;
            f.sql("ALTER SEQUENCE enforcement_status_attempts RESTART WITH 1")
                .await;
            assert_refusal(
                &f.action(action, SELLER, key, body.clone()).await,
                StatusCode::CONFLICT,
                "TRADE_FROZEN",
            );
            assert_eq!(f.snapshot().await, before, "{action}");
            let called: bool =
                sqlx::query_scalar("SELECT is_called FROM enforcement_status_attempts")
                    .fetch_one(&mut f.admin)
                    .await
                    .unwrap();
            assert!(
                !called,
                "the application check must precede the status UPDATE"
            );
            f.clear().await;
            let (s, b) = f.action(action, SELLER, key, body.clone()).await;
            assert_eq!(s, StatusCode::OK, "{action}: {b}");
            assert_clear(&b);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn delivered_unpaid_trade_cannot_be_paid_while_frozen() {
    let mut f = Fixture::new("DELIVERED").await;
    f.freeze().await;
    let before = f.snapshot().await;
    assert_refusal(
        &f.action("pay", SELLER, Some("payment"), Value::Null).await,
        StatusCode::CONFLICT,
        "TRADE_FROZEN",
    );
    assert_eq!(f.snapshot().await, before);
    assert_eq!(before["domain"]["trades"][0]["paid_at"], Value::Null);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn authority_and_validation_precede_freeze_refusal() {
    for state in ["PENDING_CONFIRMATION", "CONFIRMED", "DELIVERED"] {
        let mut f = Fixture::new(state).await;
        f.freeze().await;
        let before = f.snapshot().await;
        let action = match state {
            "PENDING_CONFIRMATION" => "confirm",
            "CONFIRMED" => "deliver",
            _ => "pay",
        };
        assert_refusal(
            &f.action(
                action,
                STRANGER,
                None,
                if action == "deliver" {
                    delivery()
                } else {
                    Value::Null
                },
            )
            .await,
            StatusCode::FORBIDDEN,
            "TRADE_PARTY_FORBIDDEN",
        );
        if state == "CONFIRMED" {
            for (body, code) in [
                (
                    json!({"final_quantity_mt":"4","final_price_per_mt":"100"}),
                    "DELIVERY_QUANTITY_INVALID",
                ),
                (
                    json!({"final_quantity_mt":"3","final_price_per_mt":"111"}),
                    "DELIVERY_PRICE_DEVIATION",
                ),
            ] {
                assert_refusal(
                    &f.action("deliver", SELLER, None, body).await,
                    StatusCode::BAD_REQUEST,
                    code,
                );
            }
        }
        let wrong = if state == "PENDING_CONFIRMATION" {
            "deliver"
        } else {
            "confirm"
        };
        assert_refusal(
            &f.action(
                wrong,
                SELLER,
                None,
                if wrong == "deliver" {
                    delivery()
                } else {
                    Value::Null
                },
            )
            .await,
            StatusCode::BAD_REQUEST,
            if wrong == "deliver" {
                "TRADE_NOT_CONFIRMED"
            } else {
                "TRADE_NOT_PENDING"
            },
        );
        assert_eq!(f.snapshot().await, before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn ordinary_confirm_requires_the_counterparty() {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    let before = f.snapshot().await;
    let result = f.action("confirm", STRANGER, None, Value::Null).await;
    assert_eq!(result.0, StatusCode::FORBIDDEN, "{}", result.1);
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_trigger_blocks_a_non_rust_writer() {
    for target in ["CONFIRMED", "DECLINED", "DELIVERED", "PAID", "CANCELLED"] {
        let mut f = Fixture::new("PENDING_CONFIRMATION").await;
        f.freeze().await;
        let before = f.snapshot().await;
        let mut writer = PgConnection::connect(&f.runtime_url).await.unwrap();
        let mut tx = writer.begin().await.unwrap();
        sqlx::query("UPDATE inventory_items SET current_stock_mt=current_stock_mt+1")
            .execute(&mut *tx)
            .await
            .unwrap();
        let error = sqlx::query("UPDATE trades SET status=$2 WHERE id=$1")
            .bind(f.trade)
            .bind(target)
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("ZF001")
        );
        tx.rollback().await.unwrap();
        assert_eq!(f.snapshot().await, before);
        println!("DIRECT_SQL target={target} SQLSTATE=ZF001 rollback=unchanged");
        sqlx::query("UPDATE trades SET status=status WHERE id=$1")
            .bind(f.trade)
            .execute(&mut writer)
            .await
            .unwrap();
        f.clear().await;
        sqlx::query("UPDATE trades SET status='CONFIRMED',confirmed_at=now() WHERE id=$1")
            .bind(f.trade)
            .execute(&mut writer)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn zf001_maps_to_trade_frozen() {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    f.sql("CREATE TRIGGER enforcement_force_frozen BEFORE UPDATE OF status ON trades FOR EACH ROW EXECUTE FUNCTION enforcement_raise_frozen()").await;
    let before = f.snapshot().await;
    let result = f
        .action("confirm", SELLER, Some("trigger"), Value::Null)
        .await;
    assert_refusal(&result, StatusCode::CONFLICT, "TRADE_FROZEN");
    assert!(!result.1.to_string().contains("ZF001"));
    assert!(!result.1.to_string().contains("synthetic status guard"));
    assert_eq!(f.snapshot().await, before);
    f.sql("DROP TRIGGER enforcement_force_frozen ON trades")
        .await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn order_cancel_preserves_frozen_pending_trade_and_reservation() {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    f.freeze().await;
    let before = f.snapshot().await;
    let result = send(
        &f.router,
        "DELETE",
        &format!("/api/orderbook/{}", id(1)),
        SELLER,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(result.0, StatusCode::NO_CONTENT, "{}", result.1);
    let after = f.snapshot().await;
    assert_eq!(after["domain"]["trades"], before["domain"]["trades"]);
    assert_eq!(after["freezes"], before["freezes"]);
    assert_eq!(after["domain"]["orders"][0]["status"], "CANCELLED");
    let reserved: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items LIMIT 1")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(reserved, rust_decimal::Decimal::from(3));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn frozen_read_is_unchanged_and_flagged() {
    let mut f = Fixture::new("CONFIRMED").await;
    let clear = f.read(SELLER, "").await;
    assert_clear(&clear["items"][0]);
    let frozen = f.freeze().await;
    let before = f.snapshot().await;
    let b = f.read(SELLER, "").await;
    assert_active(&b["items"][0], &frozen);
    let mut a = clear["items"][0].clone();
    let mut b = b["items"][0].clone();
    for v in [&mut a, &mut b] {
        v.as_object_mut().unwrap().remove("is_frozen");
        v.as_object_mut().unwrap().remove("freeze");
    }
    assert_eq!(a, b);
    assert_eq!(f.snapshot().await, before);
    f.clear().await;
    assert_clear(&f.read(SELLER, "").await["items"][0]);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn refusal_writes_nothing() {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    f.freeze().await;
    let before = f.snapshot().await;
    for action in ["confirm", "decline"] {
        assert_refusal(
            &f.action(action, SELLER, Some(action), Value::Null).await,
            StatusCode::CONFLICT,
            "TRADE_FROZEN",
        );
    }
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn lifecycle_receipts_keep_their_historical_projection() {
    for (state, action, body) in [
        ("PENDING_CONFIRMATION", "confirm", Value::Null),
        ("PENDING_CONFIRMATION", "decline", Value::Null),
        ("CONFIRMED", "deliver", delivery()),
        ("DELIVERED", "pay", Value::Null),
    ] {
        for old in [false, true] {
            let mut f = Fixture::new(state).await;
            let (s, original) = f
                .action(action, SELLER, Some("historical"), body.clone())
                .await;
            assert_eq!(s, StatusCode::OK, "{original}");
            assert_clear(&original);
            let stored = if old {
                let mut b = original.clone();
                b.as_object_mut().unwrap().remove("is_frozen");
                b.as_object_mut().unwrap().remove("freeze");
                sqlx::query("UPDATE market_command_results SET response_body=$1")
                    .bind(&b)
                    .execute(&mut f.admin)
                    .await
                    .unwrap();
                b
            } else {
                original
            };
            // Terminal receipt history can have an ACTIVE administrative row too.
            sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id) VALUES($1,$2,'ACTIVE','OTHER','receipt history review',$3)").bind(Uuid::new_v4()).bind(f.trade).bind(id(ADMIN)).execute(&mut f.admin).await.unwrap();
            let before = f.snapshot().await;
            let replay = f
                .action(action, SELLER, Some("historical"), body.clone())
                .await;
            assert_eq!(replay, (StatusCode::OK, stored.clone()));
            assert_eq!(f.snapshot().await, before);
            let denied = f
                .action(action, STRANGER, Some("historical"), body.clone())
                .await;
            assert!(!denied.0.is_success());
            assert_ne!(denied.1, stored);
            assert_eq!(f.snapshot().await, before);
            f.clear().await;
            let before = f.snapshot().await;
            assert_eq!(
                f.action(action, SELLER, Some("historical"), body.clone())
                    .await,
                (StatusCode::OK, stored)
            );
            assert_eq!(f.snapshot().await, before);
        }
    }
}

async fn named_pool(url: &str, name: &'static str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('application_name',$1,false)")
                    .bind(name)
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}
async fn wait_for_blocker(admin: &mut PgConnection, name: &str, blocker: &str, trade: bool) -> i32 {
    for _ in 0..5000 {
        let pid:Option<i32>=sqlx::query_scalar("SELECT a.pid FROM pg_stat_activity a WHERE a.datname=current_database() AND a.application_name=$1 AND a.wait_event_type='Lock' AND (NOT $3 OR a.query LIKE 'SELECT%FROM trades%FOR UPDATE') AND EXISTS(SELECT 1 FROM pg_stat_activity b WHERE b.pid=ANY(pg_blocking_pids(a.pid)) AND b.application_name=$2)")
            .bind(name).bind(blocker).bind(trade).fetch_optional(&mut *admin).await.unwrap();
        if let Some(pid) = pid {
            return pid;
        }
        tokio::task::yield_now().await;
    }
    panic!("{name} did not wait for {blocker}");
}
async fn overlap(
    f: &mut Fixture,
    first: &'static str,
    second: &'static str,
) -> ((StatusCode, Value), (StatusCode, Value)) {
    let first_pool = named_pool(&f.runtime_url, "enforcement-first").await;
    let second_pool = named_pool(&f.runtime_url, "enforcement-second").await;
    let r1 = router(first_pool.clone());
    let r2 = router(second_pool.clone());
    let mut barrier = PgConnection::connect(&f.admin_url).await.unwrap();
    sqlx::query("SELECT set_config('application_name','enforcement-barrier',false)")
        .execute(&mut barrier)
        .await
        .unwrap();
    sqlx::query("SELECT enforcement_hold()")
        .execute(&mut barrier)
        .await
        .unwrap();
    let trade = f.trade;
    let request = |r: Router, action: &'static str| {
        tokio::spawn(async move {
            send_action(
                &r,
                trade,
                action,
                if action == "freeze" { ADMIN } else { SELLER },
                Some(action),
                if action == "freeze" {
                    json!({"reason_code":"OTHER","note":"concurrent review"})
                } else if action == "deliver" {
                    delivery()
                } else {
                    Value::Null
                },
            )
            .await
        })
    };
    let a = request(r1, first);
    let pid1 = wait_for_blocker(
        &mut f.admin,
        "enforcement-first",
        "enforcement-barrier",
        false,
    )
    .await;
    let b = request(r2, second);
    let pid2 = wait_for_blocker(
        &mut f.admin,
        "enforcement-second",
        "enforcement-first",
        true,
    )
    .await;
    // Release as soon as the exact trade-row wait is visible, within the normal lock budget.
    sqlx::query("SELECT enforcement_release()")
        .execute(&mut barrier)
        .await
        .unwrap();
    assert_ne!(pid1, pid2);
    println!(
        "OBSERVED_ROUTE_WAIT first={first} second={second} first_pid={pid1} second_pid={pid2} row=trade"
    );
    let result = (a.await.unwrap(), b.await.unwrap());
    first_pool.close().await;
    second_pool.close().await;
    result
}
fn bounded(result: &(StatusCode, Value)) -> bool {
    matches!(
        result.1["detail"]["code"].as_str(),
        Some("MARKET_BUSY" | "MARKET_TIMEOUT" | "TRANSACTION_RETRY_EXHAUSTED")
    ) && matches!(result.0.as_u16(), 409 | 503)
}
async fn assert_route_schedule(state: &str, first: &'static str, second: &'static str) {
    let mut f = Fixture::new(state).await;
    if state == "CONFIRMED" {
        f.freeze().await;
        f.clear().await;
    }
    let before = f.snapshot().await;
    let (a, b) = overlap(&mut f, first, second).await;
    assert_eq!(
        a.0,
        if first == "freeze" {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        "{a:?}"
    );
    let frozen_first = first == "freeze";
    if !bounded(&b) {
        if frozen_first {
            assert_refusal(&b, StatusCode::CONFLICT, "TRADE_FROZEN");
        } else if first == "pay" {
            assert_refusal(&b, StatusCode::CONFLICT, "TRADE_NOT_SETTLEABLE");
        } else {
            assert_eq!(b.0, StatusCode::CREATED, "{b:?}");
        }
    }
    let after = f.snapshot().await;
    let transitioned = !frozen_first;
    let expected = if !transitioned {
        state
    } else {
        match first {
            "confirm" => "CONFIRMED",
            "deliver" => "DELIVERED",
            _ => "PAID",
        }
    };
    assert_eq!(after["domain"]["trades"][0]["status"], expected);
    let transition_delta = i64::from(transitioned);
    assert_eq!(
        after["domain"]["receipts"].as_i64().unwrap(),
        before["domain"]["receipts"].as_i64().unwrap() + transition_delta
    );
    assert_eq!(
        after["domain"]["outbox"].as_i64().unwrap(),
        before["domain"]["outbox"].as_i64().unwrap() + transition_delta
    );
    let freeze_delta = i64::from(frozen_first || (!bounded(&b) && first != "pay"));
    assert_eq!(
        after["domain"]["audit"].as_i64().unwrap(),
        before["domain"]["audit"].as_i64().unwrap() + transition_delta + freeze_delta
    );
    assert_eq!(
        after["domain"]["notifications"].as_i64().unwrap(),
        before["domain"]["notifications"].as_i64().unwrap() + 2 * transition_delta
    );
    for field in ["orders", "benchmarks", "targets"] {
        assert_eq!(after["domain"][field], before["domain"][field], "{field}");
    }
    if first != "confirm" {
        assert_eq!(after["domain"]["inventory"], before["domain"]["inventory"]);
        assert_eq!(after["domain"]["referrals"], before["domain"]["referrals"]);
    } else {
        let reserved: rust_decimal::Decimal =
            sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items LIMIT 1")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(reserved, rust_decimal::Decimal::from(7));
        let active: i64 =
            sqlx::query_scalar("SELECT count(*) FROM referrals WHERE status='ACTIVE'")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(active, 2);
    }
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM trade_freezes WHERE status='ACTIVE'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(active, freeze_delta);
    if frozen_first && state == "DELIVERED" {
        assert_eq!(after["domain"]["trades"][0]["paid_at"], Value::Null);
    }
    if active == 1 && expected == "DELIVERED" {
        let before = f.snapshot().await;
        assert_refusal(
            &f.action("pay", SELLER, None, Value::Null).await,
            StatusCode::CONFLICT,
            "TRADE_FROZEN",
        );
        assert_eq!(f.snapshot().await, before);
    }
    println!(
        "ROUTE_SCHEDULE first={first} second={second} first_status={} second_status={} final={expected}",
        a.0, b.0
    );
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_and_freeze_serialize_in_both_orders() {
    assert_route_schedule("PENDING_CONFIRMATION", "freeze", "confirm").await;
    assert_route_schedule("PENDING_CONFIRMATION", "confirm", "freeze").await;
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn clear_refreeze_and_delivery_serialize_in_both_orders() {
    assert_route_schedule("CONFIRMED", "freeze", "deliver").await;
    assert_route_schedule("CONFIRMED", "deliver", "freeze").await;
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn pay_and_freeze_serialize_in_both_orders() {
    assert_route_schedule("DELIVERED", "freeze", "pay").await;
    assert_route_schedule("DELIVERED", "pay", "freeze").await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn receipt_collision_recovery_keeps_frozen_historical_bodies() {
    use std::sync::{Arc, atomic::Ordering};
    use verdaxis_read::market_lifecycle::{
        Delivery, LifecycleAction, lifecycle_hash, settlement_hash,
    };
    for (state, action, body) in [
        ("PENDING_CONFIRMATION", "confirm", Value::Null),
        ("PENDING_CONFIRMATION", "decline", Value::Null),
        ("CONFIRMED", "deliver", delivery()),
        ("DELIVERED", "pay", Value::Null),
    ] {
        for mode in ["old", "new", "mismatch", "absent"] {
            let mut f = Fixture::new(state).await;
            f.sql("SELECT confirm_fixture_collision_install()").await;
            let hook = Arc::new(trade_lifecycle::ReceiptCollision::default());
            let r =
                trade_lifecycle::contract_router_with_collision(f.pool.clone(), KEY, hook.clone());
            let trade = f.trade;
            let input = body.clone();
            let before = f.snapshot().await;
            let task = tokio::spawn(async move {
                send_action(&r, trade, action, SELLER, Some("collision"), input).await
            });
            tokio::time::timeout(std::time::Duration::from_secs(15), hook.entered.notified())
                .await
                .unwrap();
            f.sql("SELECT confirm_fixture_collision_remove()").await;
            f.freeze().await;
            let hash = match action {
                "confirm" => lifecycle_hash(LifecycleAction::Confirm, trade),
                "decline" => lifecycle_hash(LifecycleAction::Decline, trade),
                _ => settlement_hash(
                    trade,
                    if action == "deliver" {
                        Some(Delivery::parse(&body).unwrap())
                    } else {
                        None
                    }
                    .as_ref(),
                ),
            }
            .unwrap();
            let winner = if mode == "new" {
                json!({"historical":action,"is_frozen":false,"freeze":null})
            } else {
                json!({"historical":action})
            };
            if mode != "absent" {
                sqlx::query("SELECT confirm_fixture_receipt($1,$2,$3,$4,$5)")
                    .bind(trade)
                    .bind(format!("trade.{action}"))
                    .bind(hash)
                    .bind(mode != "mismatch")
                    .bind(&winner)
                    .execute(&mut f.admin)
                    .await
                    .unwrap();
            }
            let recovered = f.snapshot().await;
            hook.release.notify_one();
            let result = task.await.unwrap();
            if matches!(mode, "old" | "new") {
                assert_eq!(result, (StatusCode::OK, winner));
            } else {
                assert!(!result.0.is_success(), "{result:?}");
            }
            assert_eq!(hook.executions.load(Ordering::SeqCst), 1);
            assert_eq!(f.snapshot().await, recovered);
            assert_eq!(recovered["domain"]["trades"], before["domain"]["trades"]);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn trade_and_freeze_share_one_projection_statement_snapshot() {
    for clearing in [false, true] {
        let mut f = Fixture::new("CONFIRMED").await;
        let initial = if clearing {
            Some(f.freeze().await)
        } else {
            None
        };
        f.sql("ALTER TABLE trades ENABLE ROW LEVEL SECURITY; CREATE POLICY enforcement_projection ON trades FOR SELECT TO rust_app USING (enforcement_projection_barrier())").await;
        let pool = named_pool(&f.runtime_url, "enforcement-read").await;
        let r = router(pool.clone());
        let mut barrier = PgConnection::connect(&f.admin_url).await.unwrap();
        sqlx::query("SELECT set_config('application_name','enforcement-barrier',false)")
            .execute(&mut barrier)
            .await
            .unwrap();
        sqlx::query("SELECT enforcement_hold()")
            .execute(&mut barrier)
            .await
            .unwrap();
        let read = tokio::spawn(async move {
            send(&r, "GET", "/api/trades/my", SELLER, None, Value::Null).await
        });
        wait_for_blocker(
            &mut f.admin,
            "enforcement-read",
            "enforcement-barrier",
            false,
        )
        .await;
        // Commit a freeze change and a lifecycle-neutral trade field together while projection waits.
        let mut writer = PgConnection::connect(&f.admin_url).await.unwrap();
        let mut tx = writer.begin().await.unwrap();
        if clearing {
            sqlx::query("UPDATE trade_freezes SET status='CLEARED',cleared_at=now(),cleared_by_user_id=$1,clear_note='projection complete',version=2 WHERE trade_id=$2 AND status='ACTIVE'").bind(id(ADMIN)).bind(f.trade).execute(&mut *tx).await.unwrap();
        } else {
            sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id) VALUES($1,$2,'ACTIVE','OTHER','projection review',$3)").bind(Uuid::new_v4()).bind(f.trade).bind(id(ADMIN)).execute(&mut *tx).await.unwrap();
        }
        sqlx::query("UPDATE trades SET confirmed_at=confirmed_at+interval '1 day' WHERE id=$1")
            .bind(f.trade)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("SELECT enforcement_release()")
            .execute(&mut barrier)
            .await
            .unwrap();
        let (s, b) = read.await.unwrap();
        assert_eq!(s, StatusCode::OK, "{b}");
        if let Some(frozen) = initial {
            assert_active(&b["items"][0], &frozen);
        } else {
            assert_clear(&b["items"][0]);
        }
        f.sql("DROP POLICY enforcement_projection ON trades; ALTER TABLE trades DISABLE ROW LEVEL SECURITY").await;
        pool.close().await;
        let current = f.read(SELLER, "").await;
        assert_ne!(
            current["items"][0]["confirmed_at"],
            b["items"][0]["confirmed_at"]
        );
        assert_eq!(current["items"][0]["is_frozen"], !clearing);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn take_replay_and_pages_project_current_safe_freeze_fields() {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    let raw: Value = sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
        .bind(id(1))
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let terms = verdaxis_read::canonical_terms_from_row(&raw).unwrap();
    let digest = verdaxis_read::digest_terms(&terms).unwrap();
    let input = json!({"order_id":id(1),"quantity_mt":"2","expected_terms_digest":digest});
    let (s, take) = send(
        &f.router,
        "POST",
        "/api/trades/",
        BUYER,
        Some("take"),
        input.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{take}");
    assert_clear(&take);
    f.trade = take["id"].as_str().unwrap().parse().unwrap();
    let frozen = f.freeze().await;
    let before = f.snapshot().await;
    let (s, replay) = send(
        &f.router,
        "POST",
        "/api/trades/",
        BUYER,
        Some("take"),
        input,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{replay}");
    assert_active(&replay, &frozen);
    let first = f.read(BUYER, "?limit=1").await;
    let second = f.read(BUYER, "?skip=1&limit=1").await;
    assert_eq!(first["total"], 2);
    assert_eq!(second["total"], 2);
    assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);
    let items = [&first["items"][0], &second["items"][0]];
    assert_eq!(items.iter().filter(|v| v["is_frozen"] == true).count(), 1);
    for item in items {
        assert_eq!(item["seller_name"], "Anonymous");
        assert_eq!(item["commission_fee_per_mt_usd"], Value::Null);
    }
    assert_eq!(f.snapshot().await, before);
}

async fn decline_effects_after_clear(expired: bool) {
    let mut f = Fixture::new("PENDING_CONFIRMATION").await;
    // Include a slice subscriber as well as the existing PIN subscriber.
    sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,delivery_point_id,availability_window_code) SELECT $1,watchlist_id,'SLICE',market_product_code,delivery_point_id,availability_window_code FROM watchlist_targets WHERE target_type='PIN'")
        .bind(id(0x2801)).execute(&mut f.admin).await.unwrap();
    if expired {
        sqlx::query("SELECT enforcement_expire_order($1)")
            .bind(id(1))
            .execute(&mut f.admin)
            .await
            .unwrap();
    }
    f.freeze().await;
    let frozen = f.snapshot().await;
    assert_refusal(
        &f.action("decline", SELLER, Some("decline-effects"), Value::Null)
            .await,
        StatusCode::CONFLICT,
        "TRADE_FROZEN",
    );
    // Complete rows include mirrors, PINs, audits, notifications and producer evidence.
    assert_eq!(f.snapshot().await, frozen);
    f.clear().await;
    let before = f.snapshot().await;
    let opposite: Value =
        sqlx::query_scalar("SELECT to_jsonb(b) FROM live_slice_benchmarks b WHERE side='BID'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    let (status, body) = f
        .action("decline", SELLER, Some("decline-effects"), Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DECLINED");
    assert_clear(&body);
    let after = f.snapshot().await;
    let expected_status = if expired { "EXPIRED" } else { "OPEN" };
    let order = &after["domain"]["orders"][0];
    assert_eq!(order["status"], expected_status);
    let decimal = |v: &Value| -> rust_decimal::Decimal {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
            .parse()
            .unwrap()
    };
    assert_eq!(
        decimal(&order["remaining_quantity_mt"]),
        rust_decimal::Decimal::from(10)
    );
    assert_eq!(
        order["version"].as_i64().unwrap(),
        before["domain"]["orders"][0]["version"].as_i64().unwrap() + 1
    );
    let current_opposite: Value =
        sqlx::query_scalar("SELECT to_jsonb(b) FROM live_slice_benchmarks b WHERE side='BID'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(current_opposite, opposite);
    let ask: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(b) FROM live_slice_benchmarks b WHERE side='ASK'")
            .fetch_optional(&mut f.admin)
            .await
            .unwrap();
    if expired {
        assert!(ask.is_none());
    } else {
        let ask = ask.unwrap();
        assert_eq!(
            decimal(&ask["benchmark_price_per_mt_usd"]),
            rust_decimal::Decimal::from(100)
        );
        assert_eq!(
            decimal(&ask["total_remaining_quantity_mt"]),
            rust_decimal::Decimal::from(10)
        );
        assert_eq!(ask["order_count"], 1);
    }
    let pin: Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM watchlist_targets t WHERE target_type='PIN'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(pin["snapshot_status"], expected_status);
    assert_eq!(
        decimal(&pin["snapshot_remaining_quantity_mt"]),
        rust_decimal::Decimal::from(10)
    );
    assert_eq!(
        decimal(&pin["snapshot_quantity_mt"]),
        rust_decimal::Decimal::from(10)
    );
    let event: (Value,Value) = sqlx::query_as("SELECT payload,producer_evidence FROM market_event_outbox WHERE event_type='trade_declined'")
        .fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(event.0["id"], f.trade.to_string());
    assert_eq!(event.0["status"], "DECLINED");
    // An already elapsed order is absent from both public snapshots. A live
    // restoration remains visible, so only that branch invalidates public data.
    assert_eq!(
        event
            .0
            .get("_public_market_invalidation")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        !expired
    );
    let evidence = event.1;
    assert_eq!(evidence["orders"].as_array().unwrap().len(), 1);
    assert_eq!(
        evidence["orders"][0]["before"]["status"],
        "PARTIALLY_FILLED"
    );
    assert_eq!(evidence["orders"][0]["after"]["status"], expected_status);
    assert_eq!(evidence["targets"].as_array().unwrap().len(), 2);
    assert!(
        evidence["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["target_type"] == "SLICE" && t["target_id"] == id(0x2801).to_string())
    );
    let aggregate = |field: &str| {
        evidence[field]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["side"] == "ASK")
            .unwrap()
    };
    let prior = aggregate("aggregates_before");
    let next = aggregate("aggregates_after");
    assert_eq!(prior["order_count"], if expired { 0 } else { 1 });
    assert_eq!(next["order_count"], if expired { 0 } else { 1 });
    assert_eq!(
        decimal(&prior["quantity_mt"]),
        rust_decimal::Decimal::from(if expired { 0 } else { 7 })
    );
    assert_eq!(
        decimal(&next["quantity_mt"]),
        rust_decimal::Decimal::from(if expired { 0 } else { 10 })
    );
    assert_eq!(
        prior["best_price_per_mt_usd"],
        if expired { Value::Null } else { json!("100") }
    );
    assert_eq!(
        next["best_price_per_mt_usd"],
        prior["best_price_per_mt_usd"]
    );
    // SLICE_BEST_PRICE_MOVED requires a changed best; SLICE_WENT_QUIET requires
    // nonempty before and empty after. Neither condition holds in these two
    // branches. Keep the captured subscriber and both exact snapshots so the
    // committed-evidence consumer can make that decision without live reads.
    assert_eq!(
        prior["best_price_per_mt_usd"],
        next["best_price_per_mt_usd"]
    );
    assert!(!(prior["order_count"].as_u64().unwrap() > 0 && next["order_count"] == 0));
    let audits: Vec<(String,String,Value)> = sqlx::query_as("SELECT action,resource_type,changes FROM audit_logs WHERE action IN ('trade.declined','order.expired') ORDER BY action")
        .fetch_all(&mut f.admin).await.unwrap();
    let mut expected = vec![(
        "trade.declined".to_owned(),
        "trade".to_owned(),
        json!({"status":"DECLINED"}),
    )];
    if expired {
        expected.insert(
            0,
            (
                "order.expired".to_owned(),
                "order".to_owned(),
                json!({"status":"EXPIRED","released_quantity_mt":"10"}),
            ),
        );
    }
    assert_eq!(audits, expected);
    assert_eq!(
        after["domain"]["audit"].as_i64().unwrap(),
        before["domain"]["audit"].as_i64().unwrap() + if expired { 2 } else { 1 }
    );
    assert_eq!(after["domain"]["outbox"], 1);
    assert_eq!(after["domain"]["receipts"], 1);
    assert_eq!(after["domain"]["notifications"], 2);
    assert_eq!(after["domain"]["referrals"], before["domain"]["referrals"]);
    assert_eq!(after["freezes"], before["freezes"]);
    let stock: (rust_decimal::Decimal, rust_decimal::Decimal) =
        sqlx::query_as("SELECT current_stock_mt,reserved_stock_mt FROM inventory_items")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(
        stock,
        if expired {
            (10.into(), 0.into())
        } else {
            (0.into(), 10.into())
        }
    );
    assert_eq!(
        f.action("decline", SELLER, Some("decline-effects"), Value::Null)
            .await,
        (StatusCode::OK, body)
    );
    assert_eq!(f.snapshot().await, after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn cleared_decline_restores_live_order_effects_once() {
    decline_effects_after_clear(false).await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn cleared_decline_expires_elapsed_order_with_complete_effects() {
    decline_effects_after_clear(true).await;
}

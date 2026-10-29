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
use verdaxis_read::routes::market_write::trade_lifecycle::contract_router;

const KEY: &str = "synthetic-lifecycle-signing-key-at-least-32";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const MEMBER: u128 = 0x1002;
const STRANGER: u128 = 0x3001;
const ADMIN: u128 = 0x4001;
fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
struct Fixture {
    admin: PgConnection,
    pool: PgPool,
    router: Router,
    trade: Uuid,
}
impl Fixture {
    async fn new(initiated: &str, legacy: bool) -> Self {
        let a = std::env::var("RUST_CONFIRM_ADMIN_DATABASE_URL").expect("disposable admin URL");
        let r = std::env::var("RUST_CONFIRM_DATABASE_URL").expect("disposable runtime URL");
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
        sqlx::raw_sql(include_str!("fixtures/phase9_confirm.sql"))
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
        sqlx::query("SELECT set_config('confirm.policy',$1,false)")
            .bind(policy["products"].to_string())
            .execute(&mut admin)
            .await
            .unwrap();
        let trade = sqlx::query_scalar("SELECT confirm_fixture_pending($1,$2)")
            .bind(initiated)
            .bind(legacy)
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
    async fn call(&self, action: &str, actor: u128, key: Option<&str>) -> (StatusCode, Value) {
        let headers: Vec<_> = key
            .map(|key| vec![("idempotency-key", key)])
            .unwrap_or_default();
        send(
            &self.router,
            &format!("/api/trades/{}/{action}", self.trade),
            Some(actor),
            &headers,
        )
        .await
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT confirm_fixture_snapshot()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn sql(&mut self, query: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(query.to_owned()))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn change(&mut self, actor: u128, change: &str) {
        sqlx::query("SELECT trades_fixture_change($1,$2)")
            .bind(id(actor))
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn stock(&mut self) -> (String, String) {
        sqlx::query_as("SELECT current_stock_mt::text,reserved_stock_mt::text FROM inventory_items")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn order(&mut self) -> (String, String, i32) {
        sqlx::query_as(
            "SELECT status::text,remaining_quantity_mt::text,version FROM orderbook_orders",
        )
        .fetch_one(&mut self.admin)
        .await
        .unwrap()
    }
}
async fn send(
    router: &Router,
    path: &str,
    actor: Option<u128>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut request = Request::builder().method("PUT").uri(path);
    if let Some(actor) = actor {
        let now = chrono::Utc::now().timestamp();
        let token = encode(&Header::default(), &json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":"lifecycle-tests","aud":"lifecycle-tests"}), &EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
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
fn amount(text: &str) -> rust_decimal::Decimal {
    text.parse().unwrap()
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_replay_adds_no_second_activation() {
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_referrals()").await;
    let (status, body) = f.call("confirm", SELLER, Some("confirmation")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "CONFIRMED");
    let referrals: (i64, i64) =
        sqlx::query_as("SELECT count(*),count(activated_at) FROM referrals WHERE status='ACTIVE'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(referrals, (2, 2));
    let after = f.snapshot().await;
    f.change(SELLER, "kyc").await;
    let (status, replay) = f.call("confirm", SELLER, Some("confirmation")).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, body);
    assert_eq!(f.snapshot().await, after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_requires_the_counterparty() {
    for (initiated, allowed, denied) in [
        ("BUYER", SELLER, vec![BUYER, MEMBER, STRANGER]),
        ("SELLER", BUYER, vec![SELLER, MEMBER, STRANGER]),
    ] {
        let mut f = Fixture::new(initiated, false).await;
        for actor in denied {
            let before = f.snapshot().await;
            let (s, b) = f.call("confirm", actor, None).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{initiated} actor={actor}: {b}");
            assert_eq!(f.snapshot().await, before);
        }
        let (s, b) = f.call("confirm", allowed, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_consumes_reserved_stock_exactly() {
    let mut f = Fixture::new("BUYER", false).await;
    let order = f.order().await;
    let (s, b) = f.call("confirm", SELLER, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (current, reserved) = f.stock().await;
    assert_eq!(amount(&current), amount("0"));
    assert_eq!(amount(&reserved), amount("7"));
    assert_eq!(f.order().await, order);
    assert_eq!(b["buyer_id"], id(0x1000).to_string());
    assert_eq!(b["seller_id"], id(0x2000).to_string());
    assert_eq!(b["commission_fee_per_mt_usd"], "2");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_of_a_live_order_restores_the_remainder() {
    let mut f = Fixture::new("BUYER", false).await;
    let version = f.order().await.2;
    let (s, b) = f.call("decline", SELLER, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "DECLINED");
    let (status, remainder, next) = f.order().await;
    assert_eq!(status, "OPEN");
    assert_eq!(amount(&remainder), amount("10"));
    assert_eq!(next, version + 1);
    assert_eq!(amount(&f.stock().await.1), amount("10"));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_of_a_withdrawn_order_releases_without_reviving() {
    for status in ["CANCELLED", "EXPIRED"] {
        let mut f = Fixture::new("BUYER", false).await;
        sqlx::query("SELECT confirm_fixture_withdraw($1,3)")
            .bind(status)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let order = f.order().await;
        let (s, b) = f.call("decline", SELLER, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(f.order().await, order);
        let (current, reserved) = f.stock().await;
        assert_eq!(amount(&current), amount("10"));
        assert_eq!(amount(&reserved), amount("0"));
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action='order.expired'")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn elapsed_decline_releases_the_full_restored_balance() {
    let mut f = Fixture::new("BUYER", false).await;
    f.change(1, "expired").await;
    let (s, b) = f.call("decline", SELLER, Some("elapsed")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(f.order().await.0, "EXPIRED");
    assert_eq!(amount(&f.order().await.1), amount("10"));
    let (current, reserved) = f.stock().await;
    assert_eq!(amount(&current), amount("10"));
    assert_eq!(amount(&reserved), amount("0"));
    let audit: Value =
        sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='order.expired'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(audit["status"], "EXPIRED");
    let after = f.snapshot().await;
    assert_eq!(
        f.call("decline", SELLER, Some("elapsed")).await.0,
        StatusCode::OK
    );
    assert_eq!(f.snapshot().await, after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn cleanup_uses_the_recorded_party_and_legacy_membership_only_when_null() {
    for change in ["rejected", "detached", "moved", "kyc", "email", "password"] {
        let mut f = Fixture::new("BUYER", false).await;
        f.change(SELLER, change).await;
        let (s, b) = f.call("decline", SELLER, None).await;
        assert_eq!(s, StatusCode::OK, "{change}: {b}");
        assert!(!b["seller_id"].is_null());
    }
    for initiated in ["BUYER", "SELLER"] {
        let f = Fixture::new(initiated, true).await;
        let actor = if initiated == "BUYER" { SELLER } else { MEMBER };
        let (s, b) = f.call("decline", actor, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
    }
    let mut f = Fixture::new("SELLER", false).await;
    let before = f.snapshot().await;
    assert_eq!(
        f.call("decline", MEMBER, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_fresh_admission_and_both_recorded_parties_are_rechecked() {
    for actor in [BUYER, SELLER] {
        for change in [
            "rejected", "pending", "detached", "moved", "kyc", "email", "password",
        ] {
            let mut f = Fixture::new("BUYER", false).await;
            f.change(actor, change).await;
            let before = f.snapshot().await;
            let (s, b) = f.call("confirm", SELLER, None).await;
            assert_eq!(
                s,
                if actor == SELLER {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::CONFLICT
                },
                "actor={actor} {change}: {b}"
            );
            assert_eq!(f.snapshot().await, before);
        }
    }
    let mut f = Fixture::new("BUYER", false).await;
    f.change(SELLER, "kyc").await;
    let (_, b) = f.call("confirm", SELLER, None).await;
    assert_eq!(b["detail"]["code"], "KYC_REQUIRED");
    assert!(b["detail"].get("kyc_status").is_some());
    assert!(b["detail"].get("grace_expires_at").is_some());
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn support_admin_cannot_settle() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        let before = f.snapshot().await;
        let (s, b) = f.call(action, ADMIN, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
        assert_eq!(f.snapshot().await, before);
        for header in ["not-a-uuid", "00000000-0000-0000-0000-000000006000"] {
            let (s, b) = send(
                &f.router,
                &format!("/api/trades/{}/{action}", f.trade),
                Some(SELLER),
                &[("x-verdaxis-market-support-context", header)],
            )
            .await;
            assert_eq!(s, StatusCode::FORBIDDEN);
            assert_eq!(b["detail"]["code"], "MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_and_decline_are_receipted() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        let (s, b) = f.call(action, SELLER, Some("  same-key  ")).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let after = f.snapshot().await;
        let (s, replay) = f.call(action, SELLER, Some("same-key")).await;
        assert_eq!(s, StatusCode::OK, "{replay}");
        assert_eq!(b, replay);
        assert_eq!(f.snapshot().await, after);
        assert_eq!(
            f.call(action, SELLER, None).await.0,
            StatusCode::BAD_REQUEST
        );
        let (s, _) = send(
            &f.router,
            &format!("/api/trades/{}/{action}", Uuid::new_v4()),
            Some(SELLER),
            &[("idempotency-key", "same-key")],
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(f.snapshot().await, after);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn key_limits_authentication_and_path_validation_have_no_effects() {
    let mut f = Fixture::new("BUYER", false).await;
    let before = f.snapshot().await;
    for key in [" ".to_owned(), "a".repeat(256)] {
        assert_eq!(
            f.call("confirm", SELLER, Some(&key)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(&f.router, "/api/trades/not-a-uuid/confirm", None, &[])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &f.router,
            "/api/trades/not-a-uuid/confirm",
            Some(SELLER),
            &[]
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(f.snapshot().await, before);
    let (s, b) = send(
        &f.router,
        &format!("/api/trades/{}/confirm", f.trade),
        Some(SELLER),
        &[("if-match", "garbage")],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn short_reservation_and_failed_effects_roll_back_the_whole_transition() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT confirm_fixture_withdraw('CANCELLED',2)")
            .await;
        let before = f.snapshot().await;
        let (s, b) = f.call(action, SELLER, Some("short")).await;
        assert_eq!(s, StatusCode::CONFLICT, "{b}");
        assert_eq!(f.snapshot().await, before);
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT trades_fixture_reject_audit()").await;
        let before = f.snapshot().await;
        assert!(!f.call(action, SELLER, Some("audit")).await.0.is_success());
        assert_eq!(f.snapshot().await, before);
        f.sql("SELECT trades_fixture_allow_audit()").await;
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn lifecycle_effects_have_one_scoped_envelope_and_current_initiator_recipients() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        let (s, b) = send(
            &f.router,
            &format!("/api/trades/{}/{action}", f.trade),
            Some(SELLER),
            &[
                ("x-forwarded-for", "203.0.113.12"),
                ("x-request-id", &"r".repeat(100)),
            ],
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let audit=sqlx::query("SELECT action,resource_type,resource_id,changes,ip_address,request_id FROM audit_logs WHERE resource_type='trade'").fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(
            audit.get::<String, _>("action"),
            format!(
                "trade.{}",
                if action == "confirm" {
                    "confirmed"
                } else {
                    "declined"
                }
            )
        );
        assert_eq!(audit.get::<String, _>("resource_id"), f.trade.to_string());
        assert_eq!(
            audit.get::<Value, _>("changes"),
            json!({"status":b["status"]})
        );
        assert_ne!(
            audit.get::<Option<String>, _>("ip_address").as_deref(),
            Some("203.0.113.12")
        );
        assert!(audit.get::<Option<String>, _>("request_id").unwrap().len() <= 36);
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE type='ORDER_UPDATE' AND recipient_id IN ('00000000-0000-0000-0000-000000001001','00000000-0000-0000-0000-000000001002')").fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(count, 2);
        let rows=sqlx::query("SELECT event_type,aggregate_type,aggregate_id,participant_org_ids,payload FROM market_event_outbox").fetch_all(&mut f.admin).await.unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.get::<String, _>("aggregate_type"), "trade");
        assert_eq!(row.get::<String, _>("aggregate_id"), f.trade.to_string());
        let scope: Value = row.get("participant_org_ids");
        assert_eq!(scope, json!([id(0x1000), id(0x2000)]));
        let payload: Value = row.get("payload");
        assert_eq!(payload["schema_version"], 1);
        assert_eq!(payload["id"], f.trade.to_string());
        assert_eq!(payload["status"], b["status"]);
        assert!(payload.get("commission_fee_per_mt_usd").is_none());
        assert_eq!(payload.get("quantity").is_some(), action == "confirm");
        assert_eq!(payload.get("price").is_some(), action == "confirm");
    }
}

async fn wait_for_lock(admin: &mut PgConnection) {
    for _ in 0..200 {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid())").fetch_one(&mut *admin).await.unwrap();
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the independent command did not wait on a PostgreSQL lock");
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_and_decline_have_one_winner() {
    for first in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        let mut holder = f.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM organizations WHERE id='00000000-0000-0000-0000-000000001000' FOR UPDATE").execute(&mut *holder).await.unwrap();
        let r = f.router.clone();
        let path = format!("/api/trades/{}/{first}", f.trade);
        let a = tokio::spawn(async move { send(&r, &path, Some(SELLER), &[]).await });
        wait_for_lock(&mut f.admin).await;
        holder.commit().await.unwrap();
        let (s, b) = a.await.unwrap();
        assert_eq!(s, StatusCode::OK, "{b}");
        let second = if first == "confirm" {
            "decline"
        } else {
            "confirm"
        };
        assert_eq!(
            f.call(second, SELLER, None).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            amount(&f.stock().await.1),
            amount(if first == "confirm" { "7" } else { "10" })
        );
    }
    let mut f = Fixture::new("BUYER", false).await;
    let (a, b) = tokio::join!(
        f.call("confirm", SELLER, None),
        f.call("decline", SELLER, None)
    );
    assert_eq!(
        usize::from(a.0 == StatusCode::OK) + usize::from(b.0 == StatusCode::OK),
        1,
        "{a:?} {b:?}"
    );
    assert!(
        [StatusCode::BAD_REQUEST, StatusCode::CONFLICT].contains(&if a.0 == StatusCode::OK {
            b.0
        } else {
            a.0
        })
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE resource_type='trade'")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_clock_is_captured_after_the_order_lock_wait() {
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_wait_expiry()").await;
    let mut holder = f.pool.begin().await.unwrap();
    sqlx::query("SELECT confirm_fixture_hold_order()")
        .execute(&mut *holder)
        .await
        .unwrap();
    let router = f.router.clone();
    let path = format!("/api/trades/{}/decline", f.trade);
    let command = tokio::spawn(async move { send(&router, &path, Some(SELLER), &[]).await });
    wait_for_lock(&mut f.admin).await;
    tokio::time::sleep(std::time::Duration::from_millis(275)).await;
    holder.commit().await.unwrap();
    let (s, b) = command.await.unwrap();
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(f.order().await.0, "EXPIRED");
    assert_eq!(amount(&f.stock().await.1), amount("0"));
}

async fn preserving_rejection(
    f: &Fixture,
) -> Result<verdaxis_read::business::organizations::OrganizationDecision, verdaxis_read::ApiError> {
    use std::sync::Arc;
    let policy = Arc::new(
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap(),
    );
    let config=Arc::new(serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_trade_freeze","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100})).unwrap());
    verdaxis_read::business::transaction::run_ordered_write(f.pool.clone(),Arc::new(tokio::sync::Semaphore::new(8)),{
        let now=chrono::Utc::now();
        let token=encode(&Header::default(),&json!({"sub":id(ADMIN),"type":"access","iat":now.timestamp(),"exp":now.timestamp()+600,"iss":"lifecycle-tests","aud":"lifecycle-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        verdaxis_read::auth::verify_authorization(Some(&format!("Bearer {token}")),&verdaxis_read::auth::VerificationSettings::new(KEY.into(),None,"lifecycle-tests".into(),"lifecycle-tests".into()),now).unwrap()
    },verdaxis_read::business::organizations::ReviewOrganization{organization_id:id(0x2000),approve:false,reason:Some("Fixture review".into()),now:chrono::Utc::now(),policy,config}).await
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirmation_and_preserving_organization_rejection_keep_the_pending_commitment() {
    for confirm_first in [false, true] {
        let mut f = Fixture::new("BUYER", false).await;
        if confirm_first {
            let (s, b) = f.call("confirm", SELLER, None).await;
            assert_eq!(s, StatusCode::OK, "{b}");
        }
        let mut holder = f.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
            .bind(id(0x2000))
            .execute(&mut *holder)
            .await
            .unwrap();
        let pool = f.pool.clone();
        let router = f.router.clone();
        let trade = f.trade;
        let command = tokio::spawn(async move {
            let fixture = Fixture {
                admin: PgConnection::connect(
                    &std::env::var("RUST_CONFIRM_ADMIN_DATABASE_URL").unwrap(),
                )
                .await
                .unwrap(),
                pool,
                router,
                trade,
            };
            preserving_rejection(&fixture).await
        });
        wait_for_lock(&mut f.admin).await;
        holder.commit().await.unwrap();
        command.await.unwrap().unwrap();
        if confirm_first {
            let state: String = sqlx::query_scalar("SELECT status::text FROM trades WHERE id=$1")
                .bind(f.trade)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
            assert_eq!(state, "CONFIRMED");
            assert_eq!(amount(&f.stock().await.1), amount("0"));
        } else {
            let state: String = sqlx::query_scalar("SELECT status::text FROM trades WHERE id=$1")
                .bind(f.trade)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
            assert_eq!(state, "PENDING_CONFIRMATION");
            assert_eq!(amount(&f.stock().await.1), amount("3"));
            let before = f.snapshot().await;
            assert_eq!(
                f.call("confirm", SELLER, None).await.0,
                StatusCode::FORBIDDEN
            );
            assert_eq!(f.snapshot().await, before);
            let (s, b) = f.call("decline", SELLER, None).await;
            assert_eq!(s, StatusCode::OK, "{b}");
            assert_eq!(f.order().await.0, "CANCELLED");
            assert_eq!(amount(&f.stock().await.1), amount("0"));
            assert_eq!(amount(&f.stock().await.0), amount("10"));
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirmation_and_expiry_consume_only_the_pending_quantity() {
    use verdaxis_read::market_contract::stock_contract;
    for confirm_first in [false, true] {
        let mut f = Fixture::new("BUYER", false).await;
        f.change(1, "expired").await;
        if confirm_first {
            let (s, b) = f.call("confirm", SELLER, None).await;
            assert_eq!(s, StatusCode::OK, "{b}");
        }
        let row = sqlx::query("SELECT product_id,delivery_point_id FROM orderbook_orders")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        let mut conn = f.pool.acquire().await.unwrap();
        let effects = stock_contract::expire(
            &mut conn,
            row.get("product_id"),
            row.get("delivery_point_id"),
            "SPOT",
            "ASK",
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(effects.len(), 1);
        if !confirm_first {
            assert_eq!(amount(&f.stock().await.1), amount("3"));
            let (s, b) = f.call("confirm", SELLER, None).await;
            assert_eq!(s, StatusCode::OK, "{b}");
        }
        assert_eq!(f.order().await.0, "EXPIRED");
        assert_eq!(amount(&f.stock().await.0), amount("7"));
        assert_eq!(amount(&f.stock().await.1), amount("0"));
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn linked_assisted_customers_settle_without_the_recording_admins_kyc() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT confirm_fixture_assisted()").await;
        // Rebuild the pending row with the support owner in its immutable identity.
        f.trade = sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert_eq!(f.call(action, ADMIN, None).await.0, StatusCode::FORBIDDEN);
        assert_eq!(f.snapshot().await, before);
        let (s, b) = f.call(action, SELLER, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
    }
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_assisted()").await;
    f.trade = sqlx::query_scalar("SELECT confirm_fixture_pending('SELLER',false)")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    f.change(SELLER, "kyc").await;
    let before = f.snapshot().await;
    let (s, b) = f.call("confirm", BUYER, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{b}");
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn manual_and_auto_confirmed_trades_run_the_same_referral_effect() {
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_referrals()").await;
    assert_eq!(f.call("confirm", SELLER, None).await.0, StatusCode::OK);
    let manual: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referrals WHERE status='ACTIVE' AND activated_at IS NOT NULL",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(manual, 2);
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_auto_orders(); SELECT confirm_fixture_referrals()")
        .await;
    let router = verdaxis_read::routes::market_write::order_create::contract_router(
        f.pool.clone(),
        KEY,
        Default::default(),
    );
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
    let now = chrono::Utc::now().timestamp();
    let token=encode(&Header::default(),&json!({"sub":id(BUYER),"type":"access","iat":now,"exp":now+600,"iss":"create-tests","aud":"create-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let request=Request::builder().method("POST").uri("/api/orderbook").header("authorization",format!("Bearer {token}")).header("content-type","application/json").body(Body::from(json!({"side":"BID","product_id":p["id"],"delivery_point_id":p["lanes"][0],"quantity_mt":"3","price_per_mt_usd":"100"}).to_string())).unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let auto: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referrals WHERE status='ACTIVE' AND activated_at IS NOT NULL",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(auto, manual);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM trades WHERE status='CONFIRMED'")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn scalar_trades_without_an_order_and_keyed_cleanup_context_follow_their_own_rules() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT confirm_fixture_no_order()").await;
        let order = f.order().await;
        let stock = f.stock().await;
        let (s, b) = f.call(action, SELLER, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(f.order().await, order);
        assert_eq!(f.stock().await, stock);
    }
    let mut f = Fixture::new("BUYER", false).await;
    assert_eq!(
        f.call("decline", SELLER, Some("context")).await.0,
        StatusCode::OK
    );
    f.change(SELLER, "moved").await;
    let before = f.snapshot().await;
    assert_eq!(
        f.call("decline", SELLER, Some("context")).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.snapshot().await, before);
    f.change(SELLER, "detached").await;
    let before = f.snapshot().await;
    assert_eq!(
        f.call("decline", SELLER, Some("context")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn typed_confirmation_revalidates_current_terms_and_reveals_the_confirmed_handoff() {
    let fame_bid = json!({"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU"});
    let fame_ask = json!({"schema_version":1,"side":"ASK","neat_fame":true,"uco_mass_pct":100,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU","certificate_reference":"private-certificate","certificate_holder":"private-supplier","evidence_status":"PENDING","document_references":[],"certificate_valid_until":"2027-01-01","evidence_due":"BEFORE_LOADING"});
    let lng_bid = json!({"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_delivery_methods":["SHIP_TO_SHIP"]});
    let lng_ask = json!({"side":"ASK","sustainability_scheme":"ISCC_EU","scheme_certificate_number":"private-certificate","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"100","feedstock":"MANURE","production_country":"NL","wtt_ghg_gco2e_mj":"-20.25","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"physical_supplier_name":"private-supplier","pos_timing":"WITH_DELIVERY"});
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    for (profile, bid, ask, field) in [
        ("fame_v1", fame_bid, fame_ask, "fame"),
        ("lng_v1", lng_bid, lng_ask, "lng"),
    ] {
        for changed in [false, true] {
            let mut f = Fixture::new("BUYER", false).await;
            f.sql("SELECT confirm_fixture_auto_orders()").await;
            let p = policy["products"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["qualification_profile"] == profile)
                .unwrap();
            sqlx::query("SELECT trades_fixture_typed($1,$2,$3,$4)")
                .bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap())
                .bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap())
                .bind(profile)
                .bind(&ask)
                .execute(&mut f.admin)
                .await
                .unwrap();
            let normalized_bid =
                verdaxis_read::market::terms::owner_response_terms(profile, &bid).unwrap();
            let normalized_ask =
                verdaxis_read::market::terms::owner_response_terms(profile, &ask).unwrap();
            sqlx::query("SELECT set_config($1,$2,false)")
                .bind(format!("confirm.{field}_snapshot"))
                .bind(
                    json!({"schema_version":1,"bid":normalized_bid,"ask":normalized_ask})
                        .to_string(),
                )
                .execute(&mut f.admin)
                .await
                .unwrap();
            f.trade = sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
            if changed {
                f.change(1, "off_spec").await;
            }
            let before = f.snapshot().await;
            let (s, b) = f.call("confirm", SELLER, Some("terms")).await;
            if changed {
                assert_eq!(s, StatusCode::CONFLICT, "{profile}: {b}");
                assert_eq!(f.snapshot().await, before);
            } else {
                assert_eq!(s, StatusCode::OK, "{profile}: {b}");
                assert!(b.to_string().contains("private-certificate"));
                assert!(b.to_string().contains("private-supplier"));
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_updates_only_the_changed_benchmark_and_synchronizes_pin_display() {
    for expired in [false, true] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT confirm_fixture_mirrors()").await;
        if expired {
            f.change(1, "expired").await;
        }
        let opposite: Value =
            sqlx::query_scalar("SELECT to_jsonb(b) FROM live_slice_benchmarks b WHERE side='BID'")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        let (s, b) = f.call("decline", SELLER, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let after: Value =
            sqlx::query_scalar("SELECT to_jsonb(b) FROM live_slice_benchmarks b WHERE side='BID'")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(after, opposite);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM live_slice_benchmarks WHERE side='ASK'")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(count, if expired { 0 } else { 1 });
        let target=sqlx::query("SELECT snapshot_status,snapshot_remaining_quantity_mt::numeric::text FROM watchlist_targets WHERE target_type='PIN'").fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(
            target.get::<String, _>("snapshot_status"),
            if expired { "EXPIRED" } else { "OPEN" }
        );
        assert_eq!(amount(&target.get::<String, _>(1)), amount("10"));
        let evidence: Value = sqlx::query_scalar(
            "SELECT producer_evidence FROM market_event_outbox WHERE event_type='trade_declined'",
        )
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
        assert_eq!(evidence["orders"].as_array().unwrap().len(), 1);
        assert_eq!(evidence["targets"].as_array().unwrap().len(), 1);
        assert_eq!(
            evidence["orders"][0]["before"]["status"],
            "PARTIALLY_FILLED"
        );
        assert_eq!(
            evidence["orders"][0]["after"]["status"],
            if expired { "EXPIRED" } else { "OPEN" }
        );
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn independent_inventory_changes_survive_confirmation_and_terminal_decline() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        if action == "decline" {
            f.sql("SELECT confirm_fixture_withdraw('CANCELLED',3)")
                .await;
        }
        let mut holder = f.admin.begin().await.unwrap();
        sqlx::query("SELECT confirm_fixture_shared_inventory()")
            .execute(&mut *holder)
            .await
            .unwrap();
        let router = f.router.clone();
        let path = format!("/api/trades/{}/{action}", f.trade);
        let command = tokio::spawn(async move { send(&router, &path, Some(SELLER), &[]).await });
        let mut observer =
            PgConnection::connect(&std::env::var("RUST_CONFIRM_ADMIN_DATABASE_URL").unwrap())
                .await
                .unwrap();
        wait_for_lock(&mut observer).await;
        holder.commit().await.unwrap();
        let (s, b) = command.await.unwrap();
        assert_eq!(s, StatusCode::OK, "{b}");
        let (current, reserved) = f.stock().await;
        assert_eq!(
            amount(&current),
            amount(if action == "confirm" { "100" } else { "110" })
        );
        assert_eq!(
            amount(&reserved),
            amount(if action == "confirm" { "7" } else { "0" })
        );
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn partial_and_full_live_restoration_preserve_other_confirmed_quantity() {
    for full in [false, true] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql(if full {
            "SELECT confirm_fixture_full_take()"
        } else {
            "SELECT confirm_fixture_partial_history()"
        })
        .await;
        let (s, b) = f.call("decline", SELLER, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (status, remainder, _) = f.order().await;
        assert_eq!(status, if full { "OPEN" } else { "PARTIALLY_FILLED" });
        assert_eq!(amount(&remainder), amount(if full { "10" } else { "8" }));
        assert_eq!(
            amount(&f.stock().await.1),
            amount(if full { "10" } else { "8" })
        );
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn cleanup_keeps_password_and_current_account_revocation_cutoffs() {
    for rejected in [false, true] {
        for password in [false, true] {
            let mut f = Fixture::new("BUYER", false).await;
            if rejected {
                f.change(SELLER, "rejected").await;
            }
            f.sql(if password{"UPDATE users SET password_changed_at=now()+interval '1 minute' WHERE id='00000000-0000-0000-0000-000000002001'"}else{"UPDATE users SET authentication_revoked_at=now()+interval '1 minute' WHERE id='00000000-0000-0000-0000-000000002001'"}).await;
            let before = f.snapshot().await;
            let (s, b) = f.call("decline", SELLER, None).await;
            if rejected && !password {
                assert_eq!(s, StatusCode::OK, "{b}");
            } else {
                assert_eq!(s, StatusCode::UNAUTHORIZED, "{b}");
                assert_eq!(f.snapshot().await, before);
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirmation_has_no_order_mirror_effect_and_keys_are_actor_operation_scoped() {
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_mirrors()").await;
    let before = f.snapshot().await;
    let (s, b) = f.call("confirm", SELLER, Some("scope")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let after = f.snapshot().await;
    assert_eq!(after["benchmarks"], before["benchmarks"]);
    assert_eq!(after["targets"], before["targets"]);
    // Another actor and operation do not select the seller's confirmation receipt.
    assert_eq!(
        f.call("decline", SELLER, Some("scope")).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.call("confirm", BUYER, Some("scope")).await.0,
        StatusCode::BAD_REQUEST
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM market_command_results")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn a_real_serialization_abort_retries_once_without_a_committed_prefix() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT confirm_fixture_abort_install()").await;
        let (s, b) = f.call(action, SELLER, Some("retry")).await;
        f.sql("SELECT confirm_fixture_abort_remove()").await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM confirm_fixture_attempts")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(attempts, 2);
        let after = f.snapshot().await;
        assert_eq!(after["audit"], 1);
        assert_eq!(after["outbox"], 1);
        assert_eq!(after["receipts"], 1);
        assert_eq!(
            f.call(action, SELLER, Some("retry")).await.0,
            StatusCode::OK
        );
        assert_eq!(f.snapshot().await, after);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirmation_uses_the_shared_grace_and_advisory_gate_policy() {
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_grace()").await;
    let (s, b) = f.call("confirm", SELLER, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'").await;
    f.router = contract_router(f.pool.clone(), KEY, false);
    let (s, b) = f.call("confirm", SELLER, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn assisted_link_and_customer_statuses_precede_ordinary_authority() {
    use verdaxis_read::market_lifecycle::contract_authorize;
    for change in [
        "organization",
        "recorded_user",
        "creator",
        "owner",
        "link",
        "side",
        "missing_actor",
        "missing_customer",
        "wrong_role",
        "wrong_member",
        "wrong_side",
        "valid",
    ] {
        let mut f = Fixture::new("BUYER", false).await;
        f.sql("SELECT confirm_fixture_assisted()").await;
        f.trade = sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        let expected = match change {
            "missing_actor" | "missing_customer" => StatusCode::CONFLICT,
            "valid" => StatusCode::OK,
            _ => StatusCode::FORBIDDEN,
        };
        let before = f.snapshot().await;
        let result = contract_authorize(&mut f.admin, f.trade, id(SELLER), change).await;
        assert_eq!(result, expected.as_u16(), "{change}");
        assert_eq!(f.snapshot().await, before);
    }
    // An empty current customer set blocks confirm, but not the recorded cleanup path.
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_assisted()").await;
    f.trade = sqlx::query_scalar("SELECT confirm_fixture_pending('SELLER',false)")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    f.change(SELLER, "detached").await;
    let before = f.snapshot().await;
    assert_eq!(f.call("confirm", BUYER, None).await.0, StatusCode::CONFLICT);
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn lifecycle_with_no_current_initiator_members_writes_no_notifications() {
    for action in ["confirm", "decline"] {
        let mut f = Fixture::new("BUYER", false).await;
        f.change(BUYER, "detached").await;
        f.change(MEMBER, "detached").await;
        if action == "confirm" {
            verdaxis_read::market_lifecycle::contract_empty_notification_effect(
                &mut f.admin,
                f.trade,
            )
            .await
            .unwrap();
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications")
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
            assert_eq!(count, 0);
            continue;
        }
        let (status, body) = f.call(action, SELLER, Some("empty-recipients")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(count, 0, "{action}");
        let after = f.snapshot().await;
        assert_eq!(
            f.call(action, SELLER, Some("empty-recipients")).await.0,
            StatusCode::OK
        );
        assert_eq!(f.snapshot().await, after);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_with_no_benchmark_key_leaves_existing_mirrors_unchanged() {
    let mut f = Fixture::new("BUYER", false).await;
    f.sql("SELECT confirm_fixture_mirrors()").await;
    let before = f.snapshot().await;
    verdaxis_read::market_lifecycle::contract_invalid_benchmark(&mut f.admin)
        .await
        .unwrap();
    let after = f.snapshot().await;
    assert_eq!(after["benchmarks"], before["benchmarks"]);
    assert_eq!(after["targets"], before["targets"]);
    assert_eq!(f.order().await.0, "PARTIALLY_FILLED");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn receipt_collision_replays_the_winner_and_rolls_back_staged_effects() {
    use std::sync::Arc;
    use verdaxis_read::routes::market_write::trade_lifecycle::{
        ReceiptCollision, contract_router_with_collision,
    };
    for action in ["confirm", "decline"] {
        for matching in [true, false] {
            let mut f = Fixture::new("BUYER", false).await;
            f.sql("SELECT confirm_fixture_collision_install()").await;
            let hook = Arc::new(ReceiptCollision::default());
            let router = contract_router_with_collision(f.pool.clone(), KEY, hook.clone());
            let path = format!("/api/trades/{}/{action}", f.trade);
            let before = f.snapshot().await;
            let task = tokio::spawn(async move {
                send(
                    &router,
                    &path,
                    Some(SELLER),
                    &[("idempotency-key", "collision")],
                )
                .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(10), hook.entered.notified())
                .await
                .unwrap();
            f.sql("SELECT confirm_fixture_collision_remove()").await;
            let operation = format!("trade.{action}");
            let hash = verdaxis_read::market_lifecycle::lifecycle_hash(
                if action == "confirm" {
                    verdaxis_read::market_lifecycle::LifecycleAction::Confirm
                } else {
                    verdaxis_read::market_lifecycle::LifecycleAction::Decline
                },
                f.trade,
            )
            .unwrap();
            sqlx::query("SELECT confirm_fixture_receipt($1,$2,$3,$4,$5)")
                .bind(f.trade)
                .bind(&operation)
                .bind(&hash)
                .bind(matching)
                .bind(json!({"winner":action}))
                .execute(&mut f.admin)
                .await
                .unwrap();
            hook.release.notify_one();
            let (status, body) = task.await.unwrap();
            assert_eq!(
                status,
                if matching {
                    StatusCode::OK
                } else {
                    StatusCode::CONFLICT
                },
                "{body}"
            );
            if matching {
                assert_eq!(body, json!({"winner":action}));
            }
            assert_eq!(hook.executions.load(std::sync::atomic::Ordering::SeqCst), 1);
            let mut after = f.snapshot().await;
            assert_eq!(after["receipts"], 1);
            after["receipts"] = before["receipts"].clone();
            assert_eq!(after, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn frozen_and_current_typed_terms_keep_each_status_boundary() {
    use verdaxis_read::market_lifecycle::contract_terms_status;
    let mut f = Fixture::new("BUYER", false).await;
    for profile in ["fame_v1", "lng_v1"] {
        for case in [
            "valid",
            "missing_snapshot",
            "malformed_snapshot",
            "missing_bid",
            "missing_ask",
            "wrong_side",
            "wrong_product",
            "incompatible",
            "no_order",
            "missing_current",
            "malformed_current",
            "changed_current",
            "ineligible_current",
            "scalar_no_snapshot",
        ] {
            let expected = match case {
                "valid" | "scalar_no_snapshot" => 200,
                "missing_snapshot" | "malformed_snapshot" | "missing_bid" | "missing_ask"
                | "wrong_side" | "wrong_product"
                    if profile == "lng_v1" =>
                {
                    422
                }
                _ => 409,
            };
            assert_eq!(
                contract_terms_status(&mut f.admin, profile, case).await,
                expected,
                "{profile} {case}"
            );
        }
    }
}

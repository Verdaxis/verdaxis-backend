#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use chrono::Utc;
use jsonwebtoken::{EncodingKey, Header, encode};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::order_create::{CreateSettings, contract_router};

const KEY: &str = "synthetic-create-signing-key-at-least-32-characters";
const CONTEXT: &str = "00000000-0000-0000-0000-000000003201";
fn buyer() -> Uuid {
    Uuid::from_u128(0x1001)
}
fn seller() -> Uuid {
    Uuid::from_u128(0x2001)
}
fn staff() -> Uuid {
    Uuid::from_u128(0x3001)
}
struct Fixture {
    admin: PgConnection,
    observer: PgConnection,
    pool: PgPool,
    router: Router,
    product: Uuid,
    point: Uuid,
}
impl Fixture {
    async fn new(n: i32) -> Self {
        let admin_url = std::env::var("RUST_CREATE_ADMIN_DATABASE_URL").unwrap();
        let runtime_url = std::env::var("RUST_CREATE_DATABASE_URL").unwrap();
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
        for sql in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_create.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT create_fixture_seed($1)")
            .bind(n)
            .execute(&mut admin)
            .await
            .unwrap();
        let product = sqlx::query_scalar("SELECT product_id FROM orderbook_orders LIMIT 1")
            .fetch_optional(&mut admin)
            .await
            .unwrap()
            .unwrap_or("c4cecebc-3d2d-5840-8021-57a9a11bc673".parse().unwrap());
        let point = sqlx::query_scalar("SELECT id FROM delivery_points WHERE name='Rotterdam'")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        let pool = PgPool::connect(&runtime_url).await.unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        let router = contract_router(pool.clone(), KEY, CreateSettings::default());
        Self {
            admin,
            observer,
            pool,
            router,
            product,
            point,
        }
    }
    fn body(&self, side: &str) -> Value {
        let mut b = json!({"side":side,"product_id":self.product,"delivery_point_id":self.point,"quantity_mt":"200","price_per_mt_usd":"110"});
        if side == "ASK" {
            for (k, v) in [
                ("certification_declared", json!(true)),
                ("certification_scheme", json!("ISCC EU")),
                ("specification_standard", json!("ISO 8217:2024 RF 380")),
                ("msds_available", json!(true)),
                ("carbon_intensity_gco2_mj", json!("10")),
                ("carbon_intensity_method", json!("LIFECYCLE")),
                ("feedstock", json!("residue")),
                ("origin", json!("NL")),
            ] {
                b[k] = v;
            }
        }
        b
    }
    fn assisted(&self, side: &str) -> Value {
        let mut b = self.body(side);
        b["support_confirmation"] = json!({"external_instruction_reference":" x ","instruction_at":(Utc::now()-chrono::Duration::minutes(5)).to_rfc3339(),"evidence_excerpt":" approved ","acknowledge_exact_terms":true,"acknowledge_executable_standing_order":true});
        b
    }
    async fn count(&mut self, table: &str) -> i64 {
        let sql = match table {
            "orders" => "SELECT count(*) FROM orderbook_orders",
            "trades" => "SELECT count(*) FROM trades",
            "audit" => "SELECT count(*) FROM audit_logs",
            "events" => "SELECT count(*) FROM market_event_outbox",
            "authorizations" => "SELECT count(*) FROM market_support_authorizations",
            "notifications" => "SELECT count(*) FROM notifications",
            _ => panic!("unknown table"),
        };
        sqlx::query_scalar(sql)
            .fetch_one(&mut self.observer)
            .await
            .unwrap()
    }
}
async fn send(
    router: &Router,
    actor: Option<Uuid>,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/orderbook")
        .header("content-type", "application/json");
    if let Some(actor) = actor {
        let now = Utc::now().timestamp();
        let token=encode(&Header::default(),&json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"create-tests","aud":"create-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let reply = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = reply.status();
    let bytes = axum::body::to_bytes(reply.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_order_validation_matrix() {
    let mut f = Fixture::new(0).await;
    for (actor, side) in [(seller(), "BID"), (buyer(), "ASK")] {
        let reply = send(&f.router, Some(actor), &[], f.body(side)).await;
        assert_eq!(
            reply.0,
            StatusCode::FORBIDDEN,
            "otherwise eligible wrong-role actor: {reply:?}"
        );
    }
    for (field, value, status) in [
        ("quantity_mt", json!("1"), 400),
        ("quantity_mt", json!("0"), 422),
        ("quantity_mt", json!("1.001"), 422),
        ("price_per_mt_usd", json!("1000000.01"), 422),
        ("is_anonymous", json!(false), 422),
        ("availability_window", json!("2000-01"), 400),
        ("product_id", json!(Uuid::new_v4()), 400),
        ("delivery_point_id", json!(Uuid::new_v4()), 400),
        ("expires_at", json!("2000-01-01T00:00:00Z"), 400),
        ("certification_declared", json!(true), 400),
    ] {
        let mut body = f.body("BID");
        body[field] = value;
        assert_eq!(
            send(&f.router, Some(buyer()), &[], body).await.0.as_u16(),
            status,
            "{field}"
        );
    }
    for field in [
        "certification_declared",
        "certification_scheme",
        "specification_standard",
        "msds_available",
        "carbon_intensity_gco2_mj",
        "feedstock",
        "origin",
    ] {
        let mut body = f.body("ASK");
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            send(&f.router, Some(seller()), &[], body).await.0,
            StatusCode::BAD_REQUEST,
            "{field}"
        );
    }
    assert_eq!(
        send(&f.router, None, &[], f.body("BID")).await.0,
        StatusCode::UNAUTHORIZED
    );
    let reply = send(&f.router, Some(buyer()), &[], f.body("BID")).await;
    assert_eq!(reply.0, StatusCode::CREATED, "{reply:?}");
    assert!(Uuid::parse_str(reply.1["id"].as_str().unwrap()).is_ok());
    for key in [
        "organization_id",
        "owner_user_id",
        "created_by_actor_user_id",
        "support_authorization_id",
        "trade_count",
        "etag",
    ] {
        assert!(reply.1.get(key).is_none(), "{key}");
    }
    assert_eq!(f.count("orders").await, 1);
    assert_eq!(f.count("notifications").await, 0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn fresh_execution_checks_each_party_predicate_and_kyc_contract() {
    for change in [
        "pending",
        "rejected",
        "email_unverified",
        "password_change",
        "absent",
        "organization_pending",
        "kyc_pending",
        "kyc_rejected",
        "kyc_other_organization",
    ] {
        let mut f = Fixture::new(0).await;
        sqlx::query("SELECT matching_fixture_party($1,$2)")
            .bind(buyer())
            .bind(change)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let reply = send(&f.router, Some(buyer()), &[], f.body("BID")).await;
        assert_eq!(
            reply.0.as_u16(),
            if change == "absent" { 400 } else { 403 },
            "{change}: {reply:?}"
        );
        if change.starts_with("kyc_") {
            assert_eq!(reply.1["detail"]["code"], "KYC_REQUIRED");
            assert_eq!(reply.1["detail"].as_object().unwrap().len(), 4);
            assert!(reply.1["detail"].get("kyc_status").is_some());
            assert!(reply.1["detail"].get("grace_expires_at").is_some());
        }
        assert_eq!(f.count("orders").await, 0);
    }
    let mut f = Fixture::new(0).await;
    sqlx::query("SELECT matching_fixture_party($1,'kyc_pending')")
        .bind(buyer())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let settings = CreateSettings {
        kyc_execution_gate_enabled: false,
        ..Default::default()
    };
    let router = contract_router(f.pool.clone(), KEY, settings);
    assert_eq!(
        send(&router, Some(buyer()), &[], f.body("BID")).await.0,
        StatusCode::CREATED
    );
    sqlx::query("SELECT matching_fixture_party($1,'kyc_rejected')")
        .bind(buyer())
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        send(&router, Some(buyer()), &[], f.body("BID")).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_requires_a_key_when_assisted() {
    let mut f = Fixture::new(0).await;
    for key in [None, Some(""), Some("   ")] {
        let mut headers = vec![("X-Verdaxis-Market-Support-Context", CONTEXT)];
        if let Some(key) = key {
            headers.push(("Idempotency-Key", key));
        }
        assert_eq!(
            send(&f.router, Some(staff()), &headers, f.assisted("BID"))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(f.count("authorizations").await, 0);
    assert_eq!(f.count("orders").await, 0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn assisted_create_is_post_only() {
    for n in [1, 101] {
        let mut f = Fixture::new(n).await;
        let reply = send(
            &f.router,
            Some(staff()),
            &[
                ("X-Verdaxis-Market-Support-Context", CONTEXT),
                ("Idempotency-Key", "post-only"),
            ],
            f.assisted("BID"),
        )
        .await;
        assert_eq!(reply.0, StatusCode::CONFLICT, "{reply:?}");
        assert_eq!(
            reply.1["detail"]["code"],
            if n == 101 {
                "POST_ONLY_CHECK_INDETERMINATE"
            } else {
                "POST_ONLY_WOULD_CROSS"
            }
        );
        if n == 1 {
            assert_eq!(reply.1["detail"]["best_executable_opposing_price"], "100");
        }
        assert_eq!(f.count("orders").await, i64::from(n));
        for table in [
            "authorizations",
            "trades",
            "audit",
            "events",
            "notifications",
        ] {
            assert_eq!(f.count(table).await, 0, "{table}");
        }
    }
    let mut f = Fixture::new(1).await;
    sqlx::query("SELECT create_fixture_off_spec()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let body = f.assisted("BID");
    let reply = send(
        &f.router,
        Some(staff()),
        &[
            ("X-Verdaxis-Market-Support-Context", CONTEXT),
            ("Idempotency-Key", "rest"),
        ],
        body.clone(),
    )
    .await;
    assert_eq!(reply.0, StatusCode::CREATED, "{reply:?}");
    let row=sqlx::query("SELECT status::text,evidence_reference,evidence_sha256,accountable_user_id,market_support_context_id,authorization_expires_at,consumed_at FROM market_support_authorizations").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(row.get::<String, _>(0), "CONSUMED");
    assert_eq!(row.get::<String, _>(1), "x");
    assert_eq!(row.get::<Uuid, _>(3), staff());
    assert_eq!(row.get::<Uuid, _>(4), CONTEXT.parse::<Uuid>().unwrap());
    assert!(row.get::<Option<chrono::DateTime<Utc>>, _>(6).is_some());
    assert_eq!(f.count("notifications").await, 1);
    assert_eq!(f.count("audit").await, 2);
    assert_eq!(f.count("trades").await, 0);
    assert_eq!(
        send(
            &f.router,
            Some(staff()),
            &[
                ("X-Verdaxis-Market-Support-Context", CONTEXT),
                ("Idempotency-Key", "rest")
            ],
            body
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(f.count("authorizations").await, 1);
    assert_eq!(f.count("audit").await, 2);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn support_context_expiry_commits_without_create_effects() {
    let mut f = Fixture::new(0).await;
    sqlx::query("UPDATE market_support_contexts SET expires_at=now()-interval '1 second',started_at=now()-interval '2 minutes'").execute(&mut f.admin).await.unwrap();
    let reply = send(
        &f.router,
        Some(staff()),
        &[
            ("X-Verdaxis-Market-Support-Context", CONTEXT),
            ("Idempotency-Key", "expired"),
        ],
        f.assisted("BID"),
    )
    .await;
    assert_eq!(reply.0, StatusCode::CONFLICT);
    let row: (String, i32, bool) = sqlx::query_as(
        "SELECT status::text,version,ended_at IS NOT NULL FROM market_support_contexts",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(row, ("EXPIRED".into(), 2, true));
    for table in ["orders", "authorizations", "trades", "audit", "events"] {
        assert_eq!(f.count(table).await, 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_replay_returns_the_current_projection() {
    let mut f = Fixture::new(0).await;
    let body = f.body("BID");
    let first = send(
        &f.router,
        Some(buyer()),
        &[("Idempotency-Key", "current")],
        body.clone(),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED, "{first:?}");
    let id: Uuid = first.1["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("SELECT create_fixture_current($1)")
        .bind(id)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT matching_fixture_party($1,'kyc_rejected')")
        .bind(buyer())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let replay = send(
        &f.router,
        Some(buyer()),
        &[("Idempotency-Key", "current")],
        body.clone(),
    )
    .await;
    assert_eq!(replay.0, StatusCode::CREATED, "{replay:?}");
    assert_eq!(replay.1["status"], "CANCELLED");
    assert_eq!(replay.1["version"], 2);
    assert_eq!(f.count("orders").await, 1);
    assert_eq!(f.count("audit").await, 1);
    assert_eq!(f.count("events").await, 1);
    let mut changed = body.clone();
    changed["price_per_mt_usd"] = json!("111");
    assert_eq!(
        send(
            &f.router,
            Some(buyer()),
            &[("Idempotency-Key", "current")],
            changed
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    sqlx::query("SELECT matching_fixture_member($1,$2,'BUYER')")
        .bind(Uuid::from_u128(0x1000))
        .bind(Uuid::from_u128(0x1002))
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        send(
            &f.router,
            Some(Uuid::from_u128(0x1002)),
            &[("Idempotency-Key", "current")],
            body
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM market_command_results")
            .fetch_one(&mut f.observer)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn empty_self_service_key_is_unkeyed_but_blank_key_is_invalid() {
    let mut f = Fixture::new(0).await;
    for _ in 0..2 {
        assert_eq!(
            send(
                &f.router,
                Some(buyer()),
                &[("Idempotency-Key", "")],
                f.body("BID")
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        send(
            &f.router,
            Some(buyer()),
            &[("Idempotency-Key", "   ")],
            f.body("BID")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.count("orders").await, 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM orderbook_orders WHERE idempotency_key IS NOT NULL"
        )
        .fetch_one(&mut f.observer)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_with_match_commits_order_and_trades_together() {
    let mut f = Fixture::new(2).await;
    let reply = send(
        &f.router,
        Some(buyer()),
        &[
            ("Idempotency-Key", "match"),
            ("x-request-id", &"x".repeat(300)),
        ],
        f.body("BID"),
    )
    .await;
    assert_eq!(reply.0, StatusCode::CREATED, "{reply:?}");
    assert_eq!(
        reply.1["remaining_quantity_mt"]
            .as_str()
            .unwrap()
            .parse::<Decimal>()
            .unwrap(),
        Decimal::from(198)
    );
    assert_eq!(f.count("trades").await, 2);
    assert_eq!(f.count("audit").await, 3);
    assert_eq!(f.count("notifications").await, 4);
    assert_eq!(f.count("events").await, 4);
    let counts:Vec<(String,i64)>=sqlx::query_as("SELECT event_type,count(*) FROM market_event_outbox GROUP BY event_type ORDER BY event_type").fetch_all(&mut f.observer).await.unwrap();
    assert_eq!(
        counts,
        vec![
            ("order_created".into(), 1),
            ("orders_matched".into(), 1),
            ("trade_auto_matched".into(), 2)
        ]
    );
    let private:i64=sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE payload::jsonb ? 'producer_evidence' OR payload::jsonb ? 'owner_user_id'").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(private, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE char_length(request_id)=36"
        )
        .fetch_one(&mut f.observer)
        .await
        .unwrap(),
        3
    );
    let result = send(
        &f.router,
        Some(buyer()),
        &[("Idempotency-Key", "match")],
        f.body("BID"),
    )
    .await;
    assert_eq!(result.0, StatusCode::CREATED);
    assert_eq!(f.count("events").await, 4);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn match_fanout_rolls_back_the_new_order_and_every_effect() {
    let mut f = Fixture::new(101).await;
    let reply = send(&f.router, Some(buyer()), &[], f.body("BID")).await;
    assert_eq!(reply.0, StatusCode::CONFLICT, "{reply:?}");
    assert_eq!(reply.1["detail"]["code"], "MATCH_FANOUT_EXCEEDED");
    assert_eq!(f.count("orders").await, 101);
    for table in ["trades", "audit", "events", "notifications"] {
        assert_eq!(f.count(table).await, 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn authorization_collision_is_not_an_order_replay() {
    let mut f = Fixture::new(0).await;
    sqlx::query("SELECT create_fixture_authorization_collision('collision')")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let reply = send(
        &f.router,
        Some(staff()),
        &[
            ("X-Verdaxis-Market-Support-Context", CONTEXT),
            ("Idempotency-Key", "collision"),
        ],
        f.assisted("BID"),
    )
    .await;
    assert_eq!(reply.0, StatusCode::INTERNAL_SERVER_ERROR, "{reply:?}");
    assert_eq!(f.count("orders").await, 0);
    assert_eq!(f.count("authorizations").await, 1);
    assert_eq!(f.count("audit").await, 0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_order_allocates_acceptance_after_the_slice_lock() {
    let mut f = Fixture::new(0).await;
    let key = verdaxis_read::order_write::market_slice_lock_key(f.product, Some(f.point), "SPOT");
    let mut blocker = f.pool.begin().await.unwrap();
    verdaxis_read::routes::market_write::order_create::contract_hold_key(&mut blocker, key).await;
    let before: i64 = sqlx::query_scalar("SELECT create_fixture_last_ordinal()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let router = f.router.clone();
    let body = f.body("BID");
    let request = tokio::spawn(async move { send(&router, Some(buyer()), &[], body).await });
    for _ in 0..100 {
        let waits:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event='advisory' AND datname=current_database())").fetch_one(&mut f.admin).await.unwrap();
        if waits {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT create_fixture_last_ordinal()")
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        before
    );
    blocker.commit().await.unwrap();
    assert_eq!(request.await.unwrap().0, StatusCode::CREATED);
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT acceptance_ordinal FROM orderbook_orders")
            .fetch_one(&mut f.observer)
            .await
            .unwrap()
            > before
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn same_key_creates_serialize_and_commit_one_effect_set() {
    let mut f = Fixture::new(0).await;
    let mut blocker = f.pool.begin().await.unwrap();
    let key = verdaxis_read::routes::market_write::order_create::contract_key(
        Uuid::from_u128(0x1000),
        "order.create",
        "parallel",
    );
    verdaxis_read::routes::market_write::order_create::contract_hold_key(&mut blocker, key).await;
    let router = f.router.clone();
    let body = f.body("BID");
    let first = tokio::spawn(async move {
        send(
            &router,
            Some(buyer()),
            &[("Idempotency-Key", "parallel")],
            body,
        )
        .await
    });
    let mut observed = false;
    for _ in 0..100 {
        observed=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event='advisory' AND datname=current_database())").fetch_one(&mut f.admin).await.unwrap();
        if observed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(observed, "first command must reach the real key lock");
    let router = f.router.clone();
    let body = f.body("BID");
    let second = tokio::spawn(async move {
        send(
            &router,
            Some(buyer()),
            &[("Idempotency-Key", "parallel")],
            body,
        )
        .await
    });
    assert_eq!(f.count("orders").await, 0);
    blocker.commit().await.unwrap();
    let a = first.await.unwrap();
    let mut b = second.await.unwrap();
    assert_eq!(a.0, StatusCode::CREATED, "{a:?}");
    if b.0 == StatusCode::CONFLICT {
        assert_eq!(b.1["detail"]["code"], "MARKET_BUSY");
        b = send(
            &f.router,
            Some(buyer()),
            &[("Idempotency-Key", "parallel")],
            f.body("BID"),
        )
        .await;
    }
    assert_eq!(b.0, StatusCode::CREATED, "{b:?}");
    assert_eq!(a.1["id"], b.1["id"]);
    assert_eq!(f.count("orders").await, 1);
    assert_eq!(f.count("audit").await, 1);
    assert_eq!(f.count("events").await, 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn assisted_authority_and_customer_admission_are_independent() {
    for (change, status) in [
        ("capability_revoked", 404),
        ("capability_expired", 404),
        ("context_exited", 409),
        ("context_foreign", 404),
        ("organization_pending", 409),
        ("organization_unknown", 409),
        ("member_absent", 403),
        ("member_rejected", 403),
        ("member_pending", 403),
    ] {
        let mut f = Fixture::new(0).await;
        sqlx::query("SELECT create_fixture_support_change($1)")
            .bind(change)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let reply = send(
            &f.router,
            Some(staff()),
            &[
                ("X-Verdaxis-Market-Support-Context", CONTEXT),
                ("Idempotency-Key", "authority"),
            ],
            f.assisted("BID"),
        )
        .await;
        assert_eq!(reply.0.as_u16(), status, "{change}: {reply:?}");
        for table in ["orders", "authorizations", "trades", "events", "audit"] {
            assert_eq!(f.count(table).await, 0, "{change}: {table}");
        }
    }
    let mut f = Fixture::new(0).await;
    let reply = send(
        &f.router,
        Some(staff()),
        &[
            ("X-Verdaxis-Market-Support-Context", CONTEXT),
            ("Idempotency-Key", "ask"),
        ],
        f.assisted("ASK"),
    )
    .await;
    assert_eq!(reply.0, StatusCode::CREATED, "{reply:?}");
    assert_eq!(f.count("trades").await, 0);
    assert_eq!(f.count("authorizations").await, 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn minimum_lot_and_catalog_failures_keep_their_statuses() {
    for (change, status) in [
        ("minimum_zero", 409),
        ("minimum_negative", 409),
        ("product_inactive", 400),
    ] {
        let mut f = Fixture::new(0).await;
        sqlx::query("SELECT create_fixture_catalog_change($1)")
            .bind(change)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let reply = send(&f.router, Some(buyer()), &[], f.body("BID")).await;
        assert_eq!(reply.0.as_u16(), status, "{change}: {reply:?}");
        assert_eq!(f.count("orders").await, 0);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn strict_grace_allows_pending_but_never_rejection() {
    let mut f = Fixture::new(0).await;
    sqlx::query("SELECT create_fixture_grace(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        send(&f.router, Some(buyer()), &[], f.body("BID")).await.0,
        StatusCode::CREATED
    );
    sqlx::query("SELECT create_fixture_grace(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let reply = send(&f.router, Some(buyer()), &[], f.body("BID")).await;
    assert_eq!(reply.0, StatusCode::FORBIDDEN);
    assert_eq!(reply.1["detail"]["kyc_status"], "REJECTED");
    assert!(reply.1["detail"]["grace_expires_at"].is_string());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn satisfied_hundred_fills_keep_preview_overflow_without_refusal() {
    let mut f = Fixture::new(101).await;
    sqlx::query("SELECT create_fixture_minimum_one()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut body = f.body("BID");
    body["quantity_mt"] = json!("100");
    let reply = send(&f.router, Some(buyer()), &[], body).await;
    assert_eq!(reply.0, StatusCode::CREATED, "{reply:?}");
    assert_eq!(reply.1["status"], "FILLED");
    assert_eq!(f.count("trades").await, 100);
    assert_eq!(f.count("events").await, 102);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_consumes_resting_inventory_and_rolls_back_a_later_failure() {
    for fail in [false, true] {
        let mut f = Fixture::new(2).await;
        let item = Uuid::new_v4();
        sqlx::query("SELECT matching_fixture_back_inventory($1,$2)")
            .bind(Uuid::from_u128(2))
            .bind(item)
            .execute(&mut f.admin)
            .await
            .unwrap();
        if fail {
            sqlx::query("SELECT matching_fixture_reject_trade($1)")
                .bind(Uuid::from_u128(3))
                .execute(&mut f.admin)
                .await
                .unwrap();
        }
        let reply = send(
            &f.router,
            Some(buyer()),
            &[("Idempotency-Key", "stock")],
            f.body("BID"),
        )
        .await;
        assert_eq!(
            reply.0,
            if fail {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::CREATED
            },
            "{reply:?}"
        );
        let stock: Decimal =
            sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items WHERE id=$1")
                .bind(item)
                .fetch_one(&mut f.observer)
                .await
                .unwrap();
        assert_eq!(stock, if fail { Decimal::ONE } else { Decimal::ZERO });
        if fail {
            for table in ["trades", "events", "audit", "notifications"] {
                assert_eq!(f.count(table).await, 0);
            }
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM orderbook_orders WHERE inventory_item_id IS NOT NULL"
            )
            .fetch_one(&mut f.observer)
            .await
            .unwrap(),
            1
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_rebuilds_both_benchmark_sides_once() {
    let mut f = Fixture::new(2).await;
    sqlx::raw_sql(include_str!("fixtures/market_write_match_completion.sql"))
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT completion_fixture_benchmark_probe()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let reply = send(&f.router, Some(buyer()), &[], f.body("BID")).await;
    assert_eq!(reply.0, StatusCode::CREATED, "{reply:?}");
    let sides: Vec<String> =
        sqlx::query_scalar("SELECT side::text FROM live_slice_benchmarks ORDER BY side")
            .fetch_all(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(sides, vec!["BID"]);
    let calls:Vec<(String,i64)>=sqlx::query_as("SELECT operation,count(*) FROM completion_benchmark_calls GROUP BY operation ORDER BY operation").fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(calls, vec![("DELETE".into(), 2), ("INSERT".into(), 1)]);
    sqlx::query("SELECT completion_fixture_benchmark_probe_remove()")
        .execute(&mut f.admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_retries_only_server_transaction_aborts() {
    for state in ["40001", "40P01"] {
        let mut f = Fixture::new(0).await;
        sqlx::query("SELECT create_fixture_abort_once($1)")
            .bind(state)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let reply = send(
            &f.router,
            Some(buyer()),
            &[("Idempotency-Key", "retry")],
            f.body("BID"),
        )
        .await;
        sqlx::query("SELECT create_fixture_abort_remove()")
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(reply.0, StatusCode::CREATED, "{state}: {reply:?}");
        assert_eq!(f.count("orders").await, 1);
        assert_eq!(f.count("events").await, 1);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn typed_create_uses_catalog_profiles_and_refuses_reseller_admission() {
    let f = Fixture::new(0).await;
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    for profile in ["fame_v1", "lng_v1"] {
        let p = policy["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["qualification_profile"] == profile)
            .unwrap();
        let mut body = f.body("BID");
        body["product_id"] = p["id"].clone();
        body["delivery_point_id"] = p["lanes"][0].clone();
        body["quantity_mt"] = json!("1");
        let field = if profile == "fame_v1" {
            "fame_terms"
        } else {
            "lng_terms"
        };
        assert_eq!(
            send(&f.router, Some(buyer()), &[], body.clone()).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        body[field] = if profile == "fame_v1" {
            json!({"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU"})
        } else {
            json!({"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_delivery_methods":["SHIP_TO_SHIP"]})
        };
        let reply = send(&f.router, Some(buyer()), &[], body.clone()).await;
        assert_eq!(reply.0, StatusCode::CREATED, "{profile}: {reply:?}");
        body["quantity_mt"] = json!("0.99");
        assert_eq!(
            send(&f.router, Some(buyer()), &[], body).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let p = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["qualification_profile"] == "lng_v1")
        .unwrap();
    let mut body = f.body("ASK");
    body["product_id"] = p["id"].clone();
    body["delivery_point_id"] = p["lanes"][0].clone();
    body["quantity_mt"] = json!("1");
    body["lng_terms"] = json!({"side":"ASK","sustainability_scheme":"ISCC_EU","scheme_certificate_number":"synthetic-certificate","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"100","feedstock":"MANURE","production_country":"NL","wtt_ghg_gco2e_mj":"-20.25","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"physical_supplier_name":"Synthetic supplier","pos_timing":"WITH_DELIVERY"});
    let reply = send(&f.router, Some(seller()), &[], body).await;
    assert_eq!(reply.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.1["detail"]["code"], "RESELLER_ALLOCATION_REQUIRED");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_captured_clock_keeps_a_live_order_after_a_lock_wait() {
    let mut f=Fixture::new(0).await;
    let mut settings=CreateSettings::default();
    settings.auto_matching_enabled=false;
    let router=contract_router(f.pool.clone(),KEY,settings);
    let mut blocker=f.pool.begin().await.unwrap();
    let key=verdaxis_read::order_write::market_slice_lock_key(f.product,Some(f.point),"SPOT");
    verdaxis_read::routes::market_write::order_create::contract_hold_key(&mut blocker,key).await;
    let mut body=f.body("BID");
    body["expires_at"]=json!((Utc::now()+chrono::Duration::milliseconds(180)).to_rfc3339());
    let request=tokio::spawn(async move {send(&router,Some(buyer()),&[],body).await});
    let mut observed=false;
    for _ in 0..100 {
        observed=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE wait_event='advisory' AND datname=current_database())").fetch_one(&mut f.admin).await.unwrap();
        if observed {break;}
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(observed);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    blocker.commit().await.unwrap();
    let reply=request.await.unwrap();
    assert_eq!(reply.0,StatusCode::CREATED,"{reply:?}");
    assert_eq!(reply.1["status"],"OPEN");
    assert_eq!(reply.1["version"],1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_non_replay_integrity_failure_returns_no_projection() {
    let mut f=Fixture::new(0).await;
    sqlx::query("SELECT create_fixture_always_refuse()").execute(&mut f.admin).await.unwrap();
    let reply=send(&f.router,Some(buyer()),&[("Idempotency-Key","integrity")],f.body("BID")).await;
    sqlx::query("SELECT create_fixture_abort_remove()").execute(&mut f.admin).await.unwrap();
    assert_eq!(reply.0,StatusCode::INTERNAL_SERVER_ERROR,"{reply:?}");
    assert!(reply.1.get("id").is_none());
    for table in ["orders","trades","audit","events"] {assert_eq!(f.count(table).await,0);}
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn create_commit_ack_loss_does_not_repeat_the_order() {
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let mut f=Fixture::new(0).await;
    let mut target=url::Url::parse(&std::env::var("RUST_CREATE_DATABASE_URL").unwrap()).unwrap();
    target.query_pairs_mut().append_pair("sslmode","disable");
    let upstream=target.port().unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    target.set_port(Some(listener.local_addr().unwrap().port())).unwrap();
    let commits=Arc::new(AtomicUsize::new(0));let count=commits.clone();
    let proxy=tokio::spawn(async move {
        let (mut client,_)=listener.accept().await.unwrap();
        let mut server=tokio::net::TcpStream::connect(("127.0.0.1",upstream)).await.unwrap();
        let len=client.read_u32().await.unwrap();let mut startup=vec![0;len as usize-4];client.read_exact(&mut startup).await.unwrap();
        server.write_u32(len).await.unwrap();server.write_all(&startup).await.unwrap();
        let (mut cr,mut cw)=client.into_split();let (mut sr,mut sw)=server.into_split();
        let armed=Arc::new(std::sync::atomic::AtomicBool::new(false));let arm=armed.clone();
        let sender=tokio::spawn(async move {
            loop {
                let kind=cr.read_u8().await?;let len=cr.read_u32().await?;let mut bytes=vec![0;len as usize-4];cr.read_exact(&mut bytes).await?;
                if kind==b'Q' && bytes.strip_suffix(&[0]).is_some_and(|s|s.eq_ignore_ascii_case(b"COMMIT")) {arm.store(true,Ordering::SeqCst);count.fetch_add(1,Ordering::SeqCst);}
                sw.write_u8(kind).await?;sw.write_u32(len).await?;sw.write_all(&bytes).await?;
            }
            #[allow(unreachable_code)] Ok::<(),std::io::Error>(())
        });
        while let Ok(kind)=sr.read_u8().await {
            let len=sr.read_u32().await.unwrap();let mut bytes=vec![0;len as usize-4];sr.read_exact(&mut bytes).await.unwrap();
            if armed.load(Ordering::SeqCst) {if kind==b'Z' {cw.shutdown().await.unwrap();break;}}
            else {cw.write_u8(kind).await.unwrap();cw.write_u32(len).await.unwrap();cw.write_all(&bytes).await.unwrap();}
        }
        sender.abort();let _=sender.await;
    });
    let pool=sqlx::postgres::PgPoolOptions::new().min_connections(0).max_connections(1).connect_lazy(target.as_str()).unwrap();
    let router=contract_router(pool,KEY,CreateSettings::default());
    let reply=tokio::time::timeout(std::time::Duration::from_secs(20),send(&router,Some(buyer()),&[("Idempotency-Key","commit")],f.body("BID"))).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),proxy).await.unwrap().unwrap();
    assert_eq!(reply.0,StatusCode::SERVICE_UNAVAILABLE,"{reply:?}");
    assert_eq!(reply.1["detail"]["code"],"COMMIT_OUTCOME_UNKNOWN");
    assert_eq!(commits.load(Ordering::SeqCst),1);
    assert_eq!(f.count("orders").await,1);
    let replay=send(&f.router,Some(buyer()),&[("Idempotency-Key","commit")],f.body("BID")).await;
    assert_eq!(replay.0,StatusCode::CREATED,"{replay:?}");
    assert_eq!(f.count("events").await,1);
}

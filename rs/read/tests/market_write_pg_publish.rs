#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::{inventory, order_create::CreateSettings};
use verdaxis_read::{AppState, auth::VerificationSettings};

const KEY: &str = "synthetic-publication-signing-key-at-least-32-characters";
fn seller() -> Uuid {
    Uuid::from_u128(0x2001)
}
fn item() -> Uuid {
    Uuid::from_u128(0x4000)
}

struct Fixture {
    admin: PgConnection,
    observer: PgConnection,
    pool: PgPool,
    app: AppState,
    router: Router,
}
impl Fixture {
    async fn new(n: i32, stock: i64) -> Self {
        let admin_url =
            std::env::var("RUST_PUBLISH_ADMIN_DATABASE_URL").expect("disposable administrator URL");
        let runtime_url =
            std::env::var("RUST_PUBLISH_DATABASE_URL").expect("disposable runtime URL");
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
        for fixture in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_publish.sql"),
        ] {
            sqlx::raw_sql(fixture).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT publish_fixture_seed($1,$2)")
            .bind(n)
            .bind(Decimal::from(stock))
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPool::connect(&runtime_url).await.unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        let config = serde_json::from_value(json!({"environment":"test","release_sha":verdaxis_read::compiled_sha(),"migration_revision":"p89_20261022_trade_freeze","database_url":runtime_url,"db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":true})).unwrap();
        let pools = verdaxis_read::connect(&config).await.unwrap();
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let app = AppState::new(config, pools, policy).unwrap();
        let router = component(&app, CreateSettings::default());
        Self {
            admin,
            observer,
            pool,
            app,
            router,
        }
    }
    async fn sql(&mut self, sql: &'static str) {
        sqlx::raw_sql(sql).execute(&mut self.admin).await.unwrap();
    }
    async fn counts(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT count(*) FROM orderbook_orders),'trades',(SELECT count(*) FROM trades),'audit',(SELECT count(*) FROM audit_logs),'events',(SELECT count(*) FROM market_event_outbox),'notifications',(SELECT count(*) FROM notifications),'receipts',(SELECT count(*) FROM market_command_results))").fetch_one(&mut self.observer).await.unwrap()
    }
    async fn stock(&mut self) -> (Decimal, Decimal) {
        let r = sqlx::query(
            "SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1",
        )
        .bind(item())
        .fetch_one(&mut self.observer)
        .await
        .unwrap();
        (r.get(0), r.get(1))
    }
    async fn publish(&self, key: Option<&str>) -> (StatusCode, Value) {
        send(&self.router, Some(seller()), item(), key).await
    }
}
fn component(app: &AppState, settings: CreateSettings) -> Router {
    let routes = inventory::router_with_publication(
        app.clone(),
        VerificationSettings::new(
            KEY.into(),
            None,
            "publish-tests".into(),
            "publish-tests".into(),
        ),
        settings,
    );
    verdaxis_read::runtime::http::router_with(routes.with_state(()), app.clone())
}
async fn send(
    router: &Router,
    actor: Option<Uuid>,
    item: Uuid,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/inventory/{item}/publish"));
    if let Some(actor) = actor {
        let now = Utc::now().timestamp();
        let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"publish-tests","aud":"publish-tests"}),&jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
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

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_crossing_bid_fills_at_the_resting_price() {
    let mut f = Fixture::new(1, 200).await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    let trade = sqlx::query(
        "SELECT status::text AS status,price_per_mt_usd,quantity_mt,is_anonymous FROM trades",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(trade.get::<String, _>("status"), "CONFIRMED");
    assert_eq!(
        trade.get::<Decimal, _>("price_per_mt_usd"),
        Decimal::from(110)
    );
    assert_eq!(trade.get::<Decimal, _>("quantity_mt"), Decimal::from(200));
    assert!(trade.get::<bool, _>("is_anonymous"));
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::ZERO));
    assert_eq!(
        f.counts().await,
        json!({"orders":2,"trades":1,"audit":2,"events":3,"notifications":2,"receipts":0})
    );
    let audit: Value =
        sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='inventory.published'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(audit["inventory_item_id"], item().to_string());
    assert_eq!(audit["listing_id"], reply.1["listing_id"]);
    assert_eq!(audit["status"], "FILLED");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_reserves_every_current_tonne() {
    let mut f = Fixture::new(0, 250).await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(250)));
    let row: Value = sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    for (field, value) in [
        ("side", json!("ASK")),
        ("availability_window", json!("SPOT")),
        ("inventory_item_id", json!(item())),
        ("owner_user_id", json!(seller())),
        ("created_by_actor_user_id", json!(seller())),
        ("provenance", json!("REAL")),
        ("certifications", json!([])),
        ("expires_at", Value::Null),
    ] {
        assert_eq!(row[field], value, "{field}");
    }
    assert_eq!(row["carbon_intensity_gco2_mj"], json!(0));
    assert_eq!(
        f.counts().await,
        json!({"orders":1,"trades":0,"audit":1,"events":1,"notifications":0,"receipts":0})
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_is_idempotent_per_item() {
    let mut f = Fixture::new(0, 200).await;
    let first = f.publish(Some("original")).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    let before = f.counts().await;
    f.sql("UPDATE inventory_items SET price_per_mt_usd=NULL,current_stock_mt=0,product_name='unmapped'").await;
    assert_eq!(f.publish(None).await, first);
    assert_eq!(f.publish(Some("reuse")).await, first);
    assert_eq!(f.counts().await, before);
    let reused: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE idempotency_key='reuse'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(reused, 0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn keyed_publish_replays_original_identity_after_full_fill() {
    let mut f = Fixture::new(1, 200).await;
    let first = f.publish(Some("filled")).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    let before = f.counts().await;
    assert_eq!(f.publish(Some("filled")).await, first);
    assert_eq!(f.publish(None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_replays_before_role_and_kyc() {
    let mut f = Fixture::new(1, 200).await;
    let first = f.publish(Some("principal")).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    f.sql("UPDATE users SET role='BUYER',kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    let before = f.counts().await;
    assert_eq!(f.publish(Some("principal")).await, first);
    assert_eq!(f.publish(None).await.0, StatusCode::FORBIDDEN);
    f.sql("SELECT matching_fixture_member('00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002002','SUPPLIER')").await;
    let other = send(
        &f.router,
        Some(Uuid::from_u128(0x2002)),
        item(),
        Some("principal"),
    )
    .await;
    assert_eq!(other.0, StatusCode::FORBIDDEN);
    assert_eq!(other.1["detail"]["code"], "REPLAY_PRINCIPAL_MISMATCH");
    assert_eq!(
        send(&f.router, Some(seller()), Uuid::new_v4(), Some("principal"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_requires_locked_supplier_eligibility() {
    for change in [
        "role_admin",
        "pending",
        "email_unverified",
        "password_change",
        "organization_pending",
        "kyc_rejected",
        "kyc_other_organization",
        "moved",
    ] {
        let mut f = Fixture::new(0, 200).await;
        sqlx::query("SELECT matching_fixture_party($1,$2)")
            .bind(seller())
            .bind(change)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.counts().await;
        let reply = f.publish(None).await;
        assert_eq!(reply.0, StatusCode::FORBIDDEN, "{change}: {reply:?}");
        assert_eq!(f.counts().await, before);
        assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_authentication_and_organization_precede_lookup() {
    let mut f = Fixture::new(0, 200).await;
    assert_eq!(
        send(&f.router, None, Uuid::new_v4(), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.sql("UPDATE users SET organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'")
        .await;
    assert_eq!(f.publish(None).await.0, StatusCode::BAD_REQUEST);
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    assert_eq!(
        send(&f.router, Some(seller()), Uuid::new_v4(), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_kyc_gate_and_grace_use_shared_policy() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'").await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::FORBIDDEN);
    let detail = reply.1["detail"].as_object().unwrap();
    assert_eq!(detail.len(), 4);
    assert_eq!(detail["code"], "KYC_REQUIRED");
    assert_eq!(detail["kyc_status"], "PENDING");
    assert!(detail["grace_expires_at"].is_null());
    let settings = CreateSettings {
        kyc_execution_gate_enabled: false,
        ..Default::default()
    };
    assert_eq!(
        send(&component(&f.app, settings), Some(seller()), item(), None)
            .await
            .0,
        StatusCode::OK
    );
    f.sql("UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'")
        .await;
    let settings = CreateSettings {
        kyc_execution_gate_enabled: false,
        ..Default::default()
    };
    assert_eq!(
        send(&component(&f.app, settings), Some(seller()), item(), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut f = Fixture::new(0, 200).await;
    f.sql("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'; INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002000','PENDING',now(),now()+interval '1 hour','fixture')").await;
    assert_eq!(f.publish(None).await.0, StatusCode::OK);
    f.sql("UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'")
        .await;
    assert_eq!(f.publish(None).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_header_uses_the_normalized_length() {
    let mut f = Fixture::new(0, 200).await;
    for key in [" ".to_owned(), "a".repeat(256)] {
        assert_eq!(f.publish(Some(&key)).await.0, StatusCode::BAD_REQUEST);
    }
    let key = format!("  {}  ", "a".repeat(255));
    assert_eq!(f.publish(Some(&key)).await.0, StatusCode::OK);
    let before = f.counts().await;
    assert_eq!(f.publish(Some("")).await.0, StatusCode::OK);
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_validation_has_no_partial_effect() {
    for (sql, status) in [
        ("UPDATE inventory_items SET price_per_mt_usd=NULL", 400),
        ("UPDATE inventory_items SET product_name='unmapped'", 400),
        (
            "UPDATE ports SET name='not canonical' WHERE id='pub-port'",
            400,
        ),
        ("UPDATE inventory_items SET current_stock_mt=0", 400),
        ("UPDATE inventory_items SET current_stock_mt=199", 400),
        (
            "UPDATE products SET min_lot_size=0 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673'",
            409,
        ),
        ("UPDATE inventory_items SET msds_available=false", 400),
        (
            "UPDATE inventory_items SET carbon_intensity_gco2_mj=NULL",
            400,
        ),
    ] {
        let mut f = Fixture::new(0, 200).await;
        f.sql(sql).await;
        let before = f.counts().await;
        let reply = f.publish(None).await;
        assert_eq!(reply.0.as_u16(), status, "{sql}: {reply:?}");
        assert_eq!(f.counts().await, before);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_refuses_bio_lng() {
    let mut f = Fixture::new(0, 200).await;
    let product = f
        .app
        .policy
        .products
        .iter()
        .find(|p| p.qualification_profile == "lng_v1")
        .unwrap();
    let vector: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/vectors/terms/private.json"
    ))
    .unwrap();
    sqlx::query("UPDATE inventory_items SET product_name=$1,fuel_type='LNG',lng_terms=$2,carbon_intensity_gco2_mj=NULL,carbon_intensity_method=NULL")
        .bind(&product.name).bind(&vector["row"]["lng_terms"])
        .execute(&mut f.admin)
        .await
        .unwrap();
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::BAD_REQUEST, "{reply:?}");
    assert!(
        reply.1["detail"]["message"]
            .as_str()
            .unwrap()
            .contains("lng_terms")
    );
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_matching_can_be_disabled() {
    let mut f = Fixture::new(1, 200).await;
    let settings = CreateSettings {
        auto_matching_enabled: false,
        ..Default::default()
    };
    let reply = send(&component(&f.app, settings), Some(seller()), item(), None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
    assert_eq!(f.counts().await["trades"], 0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_bound_preserves_atomicity() {
    for (n, stock, status, trades) in [
        (0, 200, 200, 0),
        (1, 200, 200, 1),
        (100, 20000, 200, 100),
        (101, 20000, 200, 100),
        (101, 20200, 409, 0),
    ] {
        let mut f = Fixture::new(n, stock).await;
        let before = f.counts().await;
        let reply = f.publish(None).await;
        assert_eq!(reply.0.as_u16(), status, "n={n},stock={stock}: {reply:?}");
        assert_eq!(f.counts().await["trades"], trades);
        if status == 409 {
            assert_eq!(f.counts().await, before);
            assert_eq!(f.stock().await, (Decimal::from(stock), Decimal::ZERO));
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_expiry_releases_stock_with_separate_effects() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("SELECT publish_fixture_expired(1,true)").await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(400)));
    let expired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM orderbook_orders WHERE status::text='EXPIRED' AND version=2",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(expired, 1);
    assert_eq!(
        f.counts().await,
        json!({"orders":2,"trades":0,"audit":2,"events":2,"notifications":0,"receipts":0})
    );
    let mut f = Fixture::new(0, 200).await;
    f.sql("SELECT publish_fixture_expired(1,false)").await;
    assert_eq!(f.publish(None).await.0, StatusCode::OK);
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
    let current:Decimal=sqlx::query_scalar("SELECT current_stock_mt FROM inventory_items WHERE id='00000000-0000-0000-0000-000000004001'").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(current, Decimal::from(200));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_refuses_expired_backlog() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("SELECT publish_fixture_expired(101,true)").await;
    let before = f.counts().await;
    let stock = f.stock().await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::CONFLICT, "{reply:?}");
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stock().await, stock);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expiry_inconsistent_reservation_rolls_back_publication() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("SELECT publish_fixture_expired(1,true); UPDATE inventory_items SET reserved_stock_mt=0")
        .await;
    let before = f.counts().await;
    assert_eq!(f.publish(None).await.0, StatusCode::CONFLICT);
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_effect_failure_rolls_back_every_write() {
    let mut f = Fixture::new(1, 200).await;
    f.sql("CREATE TRIGGER publish_fixture_refuse_effect BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION publish_fixture_refuse_effect()").await;
    let before = f.counts().await;
    let reply = f.publish(Some("rollback")).await;
    f.sql("DROP TRIGGER publish_fixture_refuse_effect ON audit_logs")
        .await;
    assert!(!reply.0.is_success(), "{reply:?}");
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_slice_change_is_409() {
    let mut f = Fixture::new(0, 200).await;
    let point = f
        .app
        .policy
        .points
        .iter()
        .find(|p| p.name == "Rotterdam")
        .unwrap()
        .id;
    let product = f
        .app
        .policy
        .products
        .iter()
        .find(|p| p.specification_standard.as_deref() == Some("ISO 8217:2024 RF 380"))
        .unwrap()
        .id;
    let mut blocker = f.pool.begin().await.unwrap();
    verdaxis_read::routes::market_write::order_create::contract_hold_key(
        &mut blocker,
        verdaxis_read::order_write::market_slice_lock_key(product, Some(point), "SPOT"),
    )
    .await;
    let router = f.router.clone();
    let task = tokio::spawn(async move { send(&router, Some(seller()), item(), None).await });
    wait_for_lock(&mut f.admin).await;
    f.sql("UPDATE ports SET name='not canonical' WHERE id='pub-port'")
        .await;
    blocker.commit().await.unwrap();
    let reply = task.await.unwrap();
    assert_eq!(reply.0, StatusCode::CONFLICT, "{reply:?}");
    assert_eq!(reply.1["detail"]["code"], "MARKET_SLICE_CHANGED");
    assert_eq!(f.counts().await["orders"], 0);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

async fn wait_for_lock(admin: &mut PgConnection) {
    tokio::time::timeout(Duration::from_secs(10),async{
        loop{
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%advisory%')").fetch_one(&mut *admin).await.unwrap();
            if waiting{break}tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("publication must reach the held slice lock");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn live_reuse_key_can_publish_replenished_stock_after_fill() {
    let mut f = Fixture::new(0, 200).await;
    let first = f.publish(Some("original")).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    assert_eq!(f.publish(Some("reused")).await, first);
    f.sql("SELECT publish_fixture_change('filled')").await;
    assert_eq!(f.publish(Some("reused")).await.0, StatusCode::BAD_REQUEST);
    f.sql("UPDATE inventory_items SET current_stock_mt=200")
        .await;
    let next = f.publish(Some("reused")).await;
    assert_eq!(next.0, StatusCode::OK, "{next:?}");
    assert_ne!(next.1["listing_id"], first.1["listing_id"]);
    assert_eq!(f.counts().await["audit"], 2);
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn matching_skips_unqualified_and_unadmitted_crossing_bids() {
    for change in [
        "SELECT publish_fixture_change('incompatible')",
        "UPDATE users SET kyc_status='REJECTED' WHERE role='BUYER'",
        "UPDATE users SET organization_id=NULL WHERE role='BUYER'",
        "SELECT publish_fixture_change('expired')",
    ] {
        let mut f = Fixture::new(1, 200).await;
        f.sql(change).await;
        let reply = f.publish(None).await;
        assert_eq!(reply.0, StatusCode::OK, "{change}: {reply:?}");
        assert_eq!(f.counts().await["trades"], 0, "{change}");
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_stages_private_events_until_commit() {
    let mut f = Fixture::new(1, 200).await;
    let barrier = std::sync::Arc::new(
        verdaxis_read::routes::market_write::order_create::CreateBarrier::default(),
    );
    let settings = CreateSettings {
        barrier: Some(barrier.clone()),
        ..Default::default()
    };
    let router = component(&f.app, settings);
    let task =
        tokio::spawn(
            async move { send(&router, Some(seller()), item(), Some("visibility")).await },
        );
    tokio::time::timeout(Duration::from_secs(10), barrier.reached.notified())
        .await
        .unwrap();
    assert_eq!(
        f.counts().await,
        json!({"orders":1,"trades":0,"audit":0,"events":0,"notifications":0,"receipts":0})
    );
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
    barrier.resume.notify_one();
    assert_eq!(task.await.unwrap().0, StatusCode::OK);
    let rows = sqlx::query(
        "SELECT event_type,participant_org_ids,payload,producer_evidence FROM market_event_outbox",
    )
    .fetch_all(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        let payload: Value = row.get("payload");
        assert!(row.get::<Option<Value>, _>("producer_evidence").is_some());
        let participants: Value = row.get("participant_org_ids");
        assert_eq!(participants.as_array().unwrap().len(), 2);
        for field in [
            "producer_evidence",
            "targets",
            "owner_user_id",
            "organization_id",
            "fame_terms",
            "lng_terms",
        ] {
            assert!(payload.get(field).is_none(), "{payload}");
        }
        if row.get::<String, _>("event_type") == "order_created" {
            assert_eq!(
                Decimal::from_str_exact(payload["quantity"].as_str().unwrap()).unwrap(),
                Decimal::ZERO
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_analytics_runs_once_after_commit() {
    use verdaxis_read::{
        analytics::sender::AnalyticsSender,
        providers::umami::{UmamiConfig, UmamiPolicy},
    };
    let deliveries = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let rows = deliveries.clone();
    let receiver = Router::new().route(
        "/api/send",
        axum::routing::post(move |axum::Json(value): axum::Json<Value>| {
            let rows = rows.clone();
            async move {
                rows.lock().await.push(value);
                axum::Json(json!({"ok":true}))
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, receiver).await.unwrap() });
    let sender = AnalyticsSender::new(
        &UmamiConfig {
            enabled: true,
            base_url: Some(format!("http://{address}")),
            website_id: Some("publication-fixture".into()),
            api_username: None,
            api_password: None,
            policy: UmamiPolicy {
                timeout: Duration::from_secs(1),
            },
            catalog: std::sync::Arc::new(Default::default()),
        },
        None,
        "https://app.example.test",
        8,
    )
    .unwrap();
    let mut f = Fixture::new(1, 200).await;
    let router = component(
        &f.app,
        CreateSettings {
            analytics: Some(sender),
            ..Default::default()
        },
    );
    let first = send(&router, Some(seller()), item(), Some("analytics")).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if deliveries.lock().await.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
    assert_eq!(f.counts().await["audit"], 2);
    assert_eq!(
        send(&router, Some(seller()), item(), Some("analytics")).await,
        first
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(deliveries.lock().await.len(), 2);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_rebuilds_each_benchmark_side_once() {
    let mut f = Fixture::new(1, 400).await;
    sqlx::raw_sql(include_str!("fixtures/market_write_match_completion.sql"))
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.sql("SELECT completion_fixture_benchmark_probe()").await;
    let reply = f.publish(None).await;
    f.sql("SELECT completion_fixture_benchmark_probe_remove()")
        .await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    let calls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM completion_benchmark_calls WHERE operation='DELETE'",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(calls, 2);
    let sides: Vec<String> =
        sqlx::query_scalar("SELECT side FROM live_slice_benchmarks ORDER BY side")
            .fetch_all(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(sides, vec!["ASK"]);
    let remaining: Decimal =
        sqlx::query_scalar("SELECT total_remaining_quantity_mt FROM live_slice_benchmarks")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(remaining, Decimal::from(200));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expiry_invalidation_follows_each_orders_public_visibility() {
    for (hidden, expected) in [(false, true), (true, false)] {
        let mut f = Fixture::new(0, 200).await;
        f.sql("SELECT publish_fixture_expired(1,false)").await;
        if hidden {
            f.sql("UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'").await;
        }
        // Keep the publisher eligible while the expired row has its own owner.
        f.sql("SELECT matching_fixture_member('00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002002','SUPPLIER')").await;
        let reply = send(&f.router, Some(Uuid::from_u128(0x2002)), item(), None).await;
        assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
        let payload: Value = sqlx::query_scalar(
            "SELECT payload::jsonb FROM market_event_outbox WHERE event_type='order_expired'",
        )
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
        assert_eq!(
            payload["_public_market_invalidation"]
                .as_bool()
                .unwrap_or(false),
            expected,
            "hidden={hidden}: {payload}"
        );
        assert_eq!(f.counts().await["audit"], 2);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn listing_expiry_during_party_lock_wait_is_not_reused() {
    let mut f = Fixture::new(0, 200).await;
    let first = f.publish(None).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    let deadline = Utc::now() + chrono::Duration::milliseconds(200);
    sqlx::query("SELECT publish_fixture_expiry($1)")
        .bind(deadline)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut lock = f.admin.begin().await.unwrap();
    sqlx::query(
        "SELECT id FROM organizations WHERE id='00000000-0000-0000-0000-000000002000' FOR UPDATE",
    )
    .execute(&mut *lock)
    .await
    .unwrap();
    let router = f.router.clone();
    let task = tokio::spawn(async move { send(&router, Some(seller()), item(), None).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    lock.commit().await.unwrap();
    let reply = task.await.unwrap();
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_ne!(reply.1["listing_id"], first.1["listing_id"]);
    let expired: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE status::text='EXPIRED'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(expired, 1);
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_integrity_error_preserves_the_original_without_a_key_row() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("CREATE TRIGGER publish_fixture_insert_error BEFORE INSERT ON orderbook_orders FOR EACH ROW EXECUTE FUNCTION publish_fixture_order_error()").await;
    let reply = f.publish(Some("no-row")).await;
    f.sql("DROP TRIGGER publish_fixture_insert_error ON orderbook_orders")
        .await;
    assert_eq!(reply.0, StatusCode::UNPROCESSABLE_ENTITY, "{reply:?}");
    assert_eq!(reply.1["detail"]["code"], "VALIDATION_FAILED");
    assert_eq!(f.counts().await["orders"], 0);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn multiple_fills_keep_each_trade_and_notification_recipient() {
    let mut f = Fixture::new(2, 500).await;
    f.sql("SELECT matching_fixture_member('00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002002','SUPPLIER')").await;
    let reply = f.publish(Some("multi")).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(100)));
    assert_eq!(
        f.counts().await,
        json!({"orders":3,"trades":2,"audit":3,"events":4,"notifications":6,"receipts":0})
    );
    let before = f.counts().await;
    assert_eq!(f.publish(Some("multi")).await, reply);
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_provenance_keeps_demo_and_unknown_orders_unmatched() {
    for kind in ["DEMO", "UNKNOWN"] {
        let mut f = Fixture::new(1, 200).await;
        sqlx::query("SELECT publish_fixture_provenance($1)")
            .bind(kind)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let reply = f.publish(None).await;
        assert_eq!(reply.0, StatusCode::OK, "{kind}: {reply:?}");
        assert_eq!(f.counts().await["trades"], 0);
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
        let row=sqlx::query("SELECT provenance::text AS provenance,expires_at FROM orderbook_orders WHERE inventory_item_id=$1").bind(item()).fetch_one(&mut f.observer).await.unwrap();
        assert_eq!(row.get::<String, _>("provenance"), kind);
        assert_eq!(
            row.get::<Option<chrono::DateTime<Utc>>, _>("expires_at")
                .is_some(),
            kind == "DEMO"
        );
    }
}

fn fame_declaration() -> Value {
    json!({"schema_version":1,"side":"ASK","neat_fame":true,"uco_mass_pct":100,"standard":"EN_14214","standard_edition":"2012+A2:2019","sustainability_scheme":"ISCC_EU","certificate_reference":"publication-certificate","certificate_holder":"publication-supplier","evidence_status":"PENDING","document_references":[],"certificate_valid_until":(Utc::now()+chrono::Duration::days(60)).date_naive().to_string(),"evidence_due":"BEFORE_LOADING"})
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn typed_fame_publication_matches_the_ordinary_create_fields() {
    let mut f = Fixture::new(0, 200).await;
    let declaration = fame_declaration();
    sqlx::query("SELECT publish_fixture_fame($1)")
        .bind(&declaration)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    let published: Value =
        sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE inventory_item_id=$1")
            .bind(item())
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    let create = verdaxis_read::routes::market_write::order_create::router(
        f.app.clone(),
        VerificationSettings::new(
            KEY.into(),
            None,
            "publish-tests".into(),
            "publish-tests".into(),
        ),
        CreateSettings {
            auto_matching_enabled: false,
            ..Default::default()
        },
    );
    let now = Utc::now().timestamp();
    let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":seller(),"type":"access","iat":now,"exp":now+600,"iss":"publish-tests","aud":"publish-tests"}),&jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let body = json!({"side":"ASK","product_id":published["product_id"],"delivery_point_id":published["delivery_point_id"],"quantity_mt":"200","price_per_mt_usd":"100","fame_terms":declaration,"certification_declared":true,"certification_scheme":"ISCC_EU","msds_available":true});
    let response = create
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orderbook")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
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
    let ordinary: Value = sqlx::query_scalar(
        "SELECT to_jsonb(o) FROM orderbook_orders o WHERE inventory_item_id IS NULL",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    for field in [
        "side",
        "product_id",
        "delivery_point_id",
        "availability_window",
        "quantity_mt",
        "remaining_quantity_mt",
        "price_per_mt_usd",
        "fame_terms",
        "certification_scheme",
        "specification_standard",
        "feedstock",
        "origin",
        "carbon_intensity_gco2_mj",
        "status",
    ] {
        assert_eq!(published[field], ordinary[field], "{field}");
    }
    assert!(ordinary["inventory_item_id"].is_null());
    assert_eq!(published["inventory_item_id"], json!(item()));
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn live_listing_future_and_null_expiry_reuse_without_effects() {
    for expiry in [None, Some(Utc::now() + chrono::Duration::hours(1))] {
        let mut f = Fixture::new(0, 200).await;
        let first = f.publish(None).await;
        assert_eq!(first.0, StatusCode::OK);
        sqlx::query("SELECT publish_fixture_expiry($1)")
            .bind(expiry)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.counts().await;
        assert_eq!(f.publish(None).await, first);
        assert_eq!(f.counts().await, before);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_updates_pinned_resting_order_and_keeps_evidence_private() {
    let mut f = Fixture::new(1, 200).await;
    sqlx::raw_sql(include_str!("fixtures/market_write_match_completion.sql"))
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.sql("SELECT completion_fixture_watch('00000000-0000-0000-0000-000000005001')")
        .await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    let row=sqlx::query("SELECT snapshot_status,snapshot_remaining_quantity_mt::numeric FROM watchlist_targets WHERE target_type='PIN'").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(row.get::<String, _>(0), "FILLED");
    assert_eq!(row.get::<Decimal, _>(1), Decimal::ZERO);
    let rows =
        sqlx::query("SELECT payload::jsonb AS payload,producer_evidence FROM market_event_outbox")
            .fetch_all(&mut f.observer)
            .await
            .unwrap();
    assert!(rows.iter().any(|row| {
        row.get::<Option<Value>, _>("producer_evidence")
            .is_some_and(|value| value.to_string().contains("target_id"))
    }));
    for row in rows {
        let payload: Value = row.get("payload");
        for field in [
            "targets",
            "target_id",
            "producer_evidence",
            "watchlist_id",
            "observer_id",
        ] {
            assert!(payload.get(field).is_none(), "{payload}");
        }
    }
}

fn hooked_component(f: &Fixture, hooks: inventory::PublicationHooks) -> Router {
    inventory::publication_contract_router(
        f.app.clone(),
        VerificationSettings::new(
            KEY.into(),
            None,
            "publish-tests".into(),
            "publish-tests".into(),
        ),
        CreateSettings::default(),
        std::sync::Arc::new(hooks),
    )
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn live_listing_equal_to_the_locked_clock_is_expired() {
    let mut f = Fixture::new(0, 200).await;
    let first = f.publish(None).await;
    assert_eq!(first.0, StatusCode::OK);
    let at = Utc::now();
    sqlx::query("SELECT publish_fixture_expiry($1)")
        .bind(at)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let router = hooked_component(
        &f,
        inventory::PublicationHooks {
            clock: Some(at),
            ..Default::default()
        },
    );
    let reply = send(&router, Some(seller()), item(), None).await;
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_ne!(reply.1["listing_id"], first.1["listing_id"]);
    assert_eq!(f.counts().await["audit"], 3);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn second_listing_check_observes_expiry_during_resource_wait() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("SELECT publish_fixture_expired(1,true)").await;
    let barrier = std::sync::Arc::new(
        verdaxis_read::routes::market_write::order_create::CreateBarrier::default(),
    );
    let router = hooked_component(
        &f,
        inventory::PublicationHooks {
            before_resources: Some(barrier.clone()),
            ..Default::default()
        },
    );
    let task = tokio::spawn(async move { send(&router, Some(seller()), item(), None).await });
    tokio::time::timeout(Duration::from_secs(10), barrier.reached.notified())
        .await
        .unwrap();
    // A retained writer can refresh the existing dated row while the request
    // is between the slice and its resource acquisition.
    let deadline = Utc::now() + chrono::Duration::milliseconds(100);
    sqlx::query("SELECT publish_fixture_expiry($1)")
        .bind(deadline)
        .execute(&mut f.admin)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    barrier.resume.notify_one();
    let reply = task.await.unwrap();
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    let expired: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE status::text='EXPIRED'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(expired, 1);
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(400)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_flush_recovers_only_a_matching_recorded_key() {
    for matching in [true, false] {
        let mut f = Fixture::new(0, 200).await;
        let barrier = std::sync::Arc::new(
            verdaxis_read::routes::market_write::order_create::CreateBarrier::default(),
        );
        let router = hooked_component(
            &f,
            inventory::PublicationHooks {
                before_recovery: Some(barrier.clone()),
                ..Default::default()
            },
        );
        f.sql("CREATE TRIGGER publish_fixture_insert_error BEFORE INSERT ON orderbook_orders FOR EACH ROW EXECUTE FUNCTION publish_fixture_order_error()").await;
        let task =
            tokio::spawn(
                async move { send(&router, Some(seller()), item(), Some("flush-key")).await },
            );
        tokio::time::timeout(Duration::from_secs(10), barrier.reached.notified())
            .await
            .unwrap();
        let hash = if matching {
            inventory::publication_contract_hash(item())
        } else {
            "a".repeat(64)
        };
        f.sql("DROP TRIGGER publish_fixture_insert_error ON orderbook_orders")
            .await;
        let listing: Uuid = sqlx::query_scalar("SELECT publish_fixture_key_row($1,$2)")
            .bind("flush-key")
            .bind(hash)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        barrier.resume.notify_one();
        let reply = task.await.unwrap();
        if matching {
            assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
            assert_eq!(reply.1["listing_id"], json!(listing));
        } else {
            assert_eq!(reply.0, StatusCode::CONFLICT, "{reply:?}");
            assert_eq!(reply.1["detail"]["code"], "IDEMPOTENCY_CONFLICT");
        }
        assert_eq!(
            f.counts().await,
            json!({"orders":1,"trades":0,"audit":0,"events":0,"notifications":0,"receipts":0})
        );
        assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn typed_publication_refuses_a_certified_fame_inventory_record() {
    let mut f = Fixture::new(0, 200).await;
    sqlx::query("SELECT publish_fixture_fame($1)")
        .bind(fame_declaration())
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.sql("UPDATE inventory_items SET is_certified=true").await;
    let before = f.counts().await;
    let reply = f.publish(None).await;
    assert_eq!(reply.0, StatusCode::UNPROCESSABLE_ENTITY, "{reply:?}");
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn unkeyed_publication_integrity_error_has_no_recovery_effects() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("CREATE TRIGGER publish_fixture_insert_error BEFORE INSERT ON orderbook_orders FOR EACH ROW EXECUTE FUNCTION publish_fixture_order_error()").await;
    let reply = f.publish(None).await;
    f.sql("DROP TRIGGER publish_fixture_insert_error ON orderbook_orders")
        .await;
    assert_eq!(reply.0, StatusCode::UNPROCESSABLE_ENTITY, "{reply:?}");
    assert_eq!(f.counts().await["orders"], 0);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn second_listing_reuse_leaves_expiry_and_stock_unchanged() {
    let mut f = Fixture::new(0, 200).await;
    f.sql("SELECT publish_fixture_expired(1,true)").await;
    let barrier = std::sync::Arc::new(
        verdaxis_read::routes::market_write::order_create::CreateBarrier::default(),
    );
    let router = hooked_component(
        &f,
        inventory::PublicationHooks {
            before_resources: Some(barrier.clone()),
            ..Default::default()
        },
    );
    let before = f.counts().await;
    let task = tokio::spawn(async move { send(&router, Some(seller()), item(), None).await });
    tokio::time::timeout(Duration::from_secs(10), barrier.reached.notified())
        .await
        .unwrap();
    sqlx::query("SELECT publish_fixture_expiry($1)")
        .bind(Utc::now() + chrono::Duration::hours(1))
        .execute(&mut f.admin)
        .await
        .unwrap();
    barrier.resume.notify_one();
    let reply = task.await.unwrap();
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_eq!(reply.1["listing_id"], json!(Uuid::from_u128(0x6001)));
    assert_eq!(f.counts().await, before);
    assert_eq!(f.stock().await, (Decimal::from(200), Decimal::from(200)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn listing_expiry_during_inventory_lock_wait_creates_a_new_listing() {
    let mut f = Fixture::new(0, 200).await;
    let first = f.publish(None).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    let deadline = Utc::now() + chrono::Duration::milliseconds(250);
    sqlx::query("SELECT publish_fixture_expiry($1)")
        .bind(deadline)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut blocker = f.pool.begin().await.unwrap();
    sqlx::query("SELECT publish_fixture_hold_item($1)")
        .bind(item())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let router = f.router.clone();
    let task = tokio::spawn(async move { send(&router, Some(seller()), item(), None).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let returned_before_unlock = task.is_finished();
    tokio::time::sleep(Duration::from_millis(250)).await;
    blocker.commit().await.unwrap();
    let reply = task.await.unwrap();
    assert!(
        !returned_before_unlock,
        "publication must wait for the inventory row"
    );
    assert_eq!(reply.0, StatusCode::OK, "{reply:?}");
    assert_ne!(reply.1["listing_id"], first.1["listing_id"]);
    let rows=sqlx::query("SELECT status::text AS status,quantity_mt,remaining_quantity_mt FROM orderbook_orders ORDER BY created_at,id").fetch_all(&mut f.observer).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String, _>("status"), "EXPIRED");
    assert_eq!(rows[1].get::<String, _>("status"), "OPEN");
    assert_eq!(
        rows[1].get::<Decimal, _>("remaining_quantity_mt"),
        Decimal::from(200)
    );
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(200)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_and_ordinary_create_share_crossing_match_outcomes() {
    for publication in [true, false] {
        let mut f = Fixture::new(1, 300).await;
        let reply = if publication {
            f.publish(None).await
        } else {
            let router = verdaxis_read::routes::market_write::order_create::router(
                f.app.clone(),
                VerificationSettings::new(
                    KEY.into(),
                    None,
                    "publish-tests".into(),
                    "publish-tests".into(),
                ),
                CreateSettings::default(),
            );
            let source: Value =
                sqlx::query_scalar("SELECT to_jsonb(i) FROM inventory_items i WHERE id=$1")
                    .bind(item())
                    .fetch_one(&mut f.observer)
                    .await
                    .unwrap();
            let bid =
                sqlx::query("SELECT product_id,delivery_point_id FROM orderbook_orders LIMIT 1")
                    .fetch_one(&mut f.observer)
                    .await
                    .unwrap();
            let body = json!({"side":"ASK","product_id":bid.get::<Uuid,_>("product_id"),"delivery_point_id":bid.get::<Uuid,_>("delivery_point_id"),"quantity_mt":"300","price_per_mt_usd":"100","certification_declared":true,"certification_scheme":source["certification_scheme"],"specification_standard":source["specification_standard"],"msds_available":true,"carbon_intensity_gco2_mj":"0","carbon_intensity_method":source["carbon_intensity_method"],"feedstock":source["feedstock"],"origin":source["origin"]});
            let now = Utc::now().timestamp();
            let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":seller(),"type":"access","iat":now,"exp":now+600,"iss":"publish-tests","aud":"publish-tests"}),&jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
            let response = router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/orderbook")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (status, serde_json::from_slice(&bytes).unwrap())
        };
        assert_eq!(
            reply.0,
            if publication {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
            "{reply:?}"
        );
        let trade =
            sqlx::query("SELECT status::text AS status,quantity_mt,price_per_mt_usd FROM trades")
                .fetch_one(&mut f.observer)
                .await
                .unwrap();
        assert_eq!(trade.get::<String, _>("status"), "CONFIRMED");
        assert_eq!(trade.get::<Decimal, _>("quantity_mt"), Decimal::from(200));
        assert_eq!(
            trade.get::<Decimal, _>("price_per_mt_usd"),
            Decimal::from(110)
        );
        let orders=sqlx::query("SELECT side::text AS side,status::text AS status,quantity_mt,remaining_quantity_mt,inventory_item_id FROM orderbook_orders ORDER BY side").fetch_all(&mut f.observer).await.unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].get::<String, _>("side"), "ASK");
        assert_eq!(orders[0].get::<String, _>("status"), "PARTIALLY_FILLED");
        assert_eq!(
            orders[0].get::<Decimal, _>("quantity_mt"),
            Decimal::from(300)
        );
        assert_eq!(
            orders[0].get::<Decimal, _>("remaining_quantity_mt"),
            Decimal::from(100)
        );
        assert_eq!(
            orders[0].get::<Option<Uuid>, _>("inventory_item_id"),
            publication.then_some(item())
        );
        assert_eq!(orders[1].get::<String, _>("status"), "FILLED");
        assert_eq!(
            orders[1].get::<Decimal, _>("remaining_quantity_mt"),
            Decimal::ZERO
        );
        assert_eq!(
            f.stock().await,
            if publication {
                (Decimal::ZERO, Decimal::from(100))
            } else {
                (Decimal::from(300), Decimal::ZERO)
            }
        );
    }
}

async fn concurrent(n: i32, stock: i64, key: Option<&str>) {
    let mut f = Fixture::new(n, stock).await;
    let mut blocker = f.pool.begin().await.unwrap();
    let product = f
        .app
        .policy
        .products
        .iter()
        .find(|p| p.name == "B30")
        .map(|p| p.id)
        .unwrap_or_else(|| {
            f.app
                .policy
                .products
                .iter()
                .find(|p| p.specification_standard.as_deref() == Some("ISO 8217:2024 RF 380"))
                .unwrap()
                .id
        });
    let point = f
        .app
        .policy
        .points
        .iter()
        .find(|p| p.name == "Rotterdam")
        .unwrap()
        .id;
    let lock = verdaxis_read::order_write::market_slice_lock_key(product, Some(point), "SPOT");
    verdaxis_read::routes::market_write::order_create::contract_hold_key(&mut blocker, lock).await;
    let router = f.router.clone();
    let owned_key = key.map(str::to_owned);
    let first =
        tokio::spawn(
            async move { send(&router, Some(seller()), item(), owned_key.as_deref()).await },
        );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let router = f.router.clone();
    let owned_key = key.map(str::to_owned);
    let second =
        tokio::spawn(
            async move { send(&router, Some(seller()), item(), owned_key.as_deref()).await },
        );
    blocker.commit().await.unwrap();
    let a = first.await.unwrap();
    let b = second.await.unwrap();
    assert!(a.0.is_success() || b.0.is_success(), "{a:?} {b:?}");
    for response in [&a, &b] {
        assert!(
            [200, 400, 409, 503].contains(&response.0.as_u16()),
            "{response:?}"
        );
    }
    let again = f.publish(key).await;
    if n == 1 && stock == 200 && key.is_none() {
        assert_eq!(again.0, StatusCode::BAD_REQUEST);
    } else {
        assert_eq!(again.0, StatusCode::OK, "{again:?}");
    }
    let counts = f.counts().await;
    assert_eq!(counts["orders"], n + 1);
    assert_eq!(counts["trades"], n);
    assert_eq!(counts["audit"], n + 1);
    assert_eq!(counts["events"], if n == 0 { 1 } else { 3 });
    assert_eq!(
        f.stock().await,
        (Decimal::ZERO, Decimal::from(stock - n as i64 * 200))
    );
}
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn double_unkeyed_publish_with_live_remainder_returns_one_listing() {
    concurrent(0, 200, None).await;
    concurrent(1, 400, None).await;
}
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn double_unkeyed_publish_after_full_fill_refuses_without_effect() {
    concurrent(1, 200, None).await;
}
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_keyed_publication_replays_original_identity() {
    concurrent(1, 200, Some("same-key")).await;
}

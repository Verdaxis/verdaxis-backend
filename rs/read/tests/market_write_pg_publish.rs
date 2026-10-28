#![cfg(feature = "contract-tests")]

use std::{sync::Arc, time::Duration};
use axum::{Router, body::Body, http::{Request, StatusCode}};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{AppState, auth::VerificationSettings};
use verdaxis_read::routes::market_write::{inventory, order_create::CreateSettings};

const KEY: &str = "synthetic-publication-signing-key-at-least-32-characters";
fn seller() -> Uuid { Uuid::from_u128(0x2001) }
fn item() -> Uuid { Uuid::from_u128(0x4000) }
fn org() -> Uuid { Uuid::from_u128(0x2000) }

struct Fixture { admin: PgConnection, observer: PgConnection, pool: PgPool, app: AppState, router: Router }
impl Fixture {
    async fn new(n: i32, stock: i64) -> Self {
        let admin_url = std::env::var("RUST_PUBLISH_ADMIN_DATABASE_URL").expect("disposable administrator URL");
        let runtime_url = std::env::var("RUST_PUBLISH_DATABASE_URL").expect("disposable runtime URL");
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert!(a.path().ends_with("_test"));
        assert_eq!((a.host_str(),a.port(),a.path()),(r.host_str(),r.port(),r.path()));
        assert_ne!(a.username(), r.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        for fixture in [include_str!("fixtures/market_write_matching.sql"),include_str!("fixtures/market_write_publish.sql")] {
            sqlx::raw_sql(fixture).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT publish_fixture_seed($1,$2)").bind(n).bind(Decimal::from(stock)).execute(&mut admin).await.unwrap();
        let pool = PgPool::connect(&runtime_url).await.unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        let config = serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_trade_freeze","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":true})).unwrap();
        let app = AppState { config:Arc::new(config),pools:verdaxis_read::runtime::db::Pools{read:pool.clone(),health:pool.clone()},policy:Arc::new(serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()),reads:Arc::new(tokio::sync::Semaphore::new(8)),reference_cache:Arc::new(Default::default()) };
        let router = component(&app, CreateSettings::default());
        Self { admin, observer, pool, app, router }
    }
    async fn sql(&mut self, sql: &str) { sqlx::raw_sql(sql).execute(&mut self.admin).await.unwrap(); }
    async fn counts(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('orders',(SELECT count(*) FROM orderbook_orders),'trades',(SELECT count(*) FROM trades),'audit',(SELECT count(*) FROM audit_logs),'events',(SELECT count(*) FROM market_event_outbox),'notifications',(SELECT count(*) FROM notifications),'receipts',(SELECT count(*) FROM market_command_results))").fetch_one(&mut self.observer).await.unwrap()
    }
    async fn stock(&mut self) -> (Decimal,Decimal) {
        let r=sqlx::query("SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1").bind(item()).fetch_one(&mut self.observer).await.unwrap();
        (r.get(0),r.get(1))
    }
    async fn publish(&self,key: Option<&str>) -> (StatusCode,Value) { send(&self.router,Some(seller()),item(),key).await }
}
fn component(app:&AppState,settings:CreateSettings)->Router {
    let routes=inventory::router_with_publication(app.clone(),VerificationSettings::new(KEY.into(),None,"publish-tests".into(),"publish-tests".into()),settings);
    verdaxis_read::runtime::http::router_with(routes.with_state(()),app.clone())
}
async fn send(router:&Router,actor:Option<Uuid>,item:Uuid,key:Option<&str>)->(StatusCode,Value) {
    let mut request=Request::builder().method("POST").uri(format!("/api/inventory/{item}/publish"));
    if let Some(actor)=actor {
        let now=Utc::now().timestamp();
        let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"publish-tests","aud":"publish-tests"}),&jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request=request.header("authorization",format!("Bearer {token}"));
    }
    if let Some(key)=key { request=request.header("idempotency-key",key); }
    let response=router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status=response.status();
    let bytes=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
    (status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_crossing_bid_fills_at_the_resting_price() {
    let mut f=Fixture::new(1,200).await;
    let reply=f.publish(None).await;
    assert_eq!(reply.0,StatusCode::OK,"{reply:?}");
    let trade=sqlx::query("SELECT status::text AS status,price_per_mt_usd,quantity_mt,is_anonymous FROM trades").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(trade.get::<String,_>("status"),"CONFIRMED");
    assert_eq!(trade.get::<Decimal,_>("price_per_mt_usd"),Decimal::from(110));
    assert_eq!(trade.get::<Decimal,_>("quantity_mt"),Decimal::from(200));
    assert!(trade.get::<bool,_>("is_anonymous"));
    assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::ZERO));
    assert_eq!(f.counts().await,json!({"orders":2,"trades":1,"audit":2,"events":3,"notifications":2,"receipts":0}));
    let audit:Value=sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='inventory.published'").fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(audit["inventory_item_id"],item().to_string());
    assert_eq!(audit["listing_id"],reply.1["listing_id"]);
    assert_eq!(audit["status"],"FILLED");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_reserves_every_current_tonne() {
    let mut f=Fixture::new(0,250).await;
    let reply=f.publish(None).await; assert_eq!(reply.0,StatusCode::OK,"{reply:?}");
    assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::from(250)));
    let row:Value=sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o").fetch_one(&mut f.observer).await.unwrap();
    for (field,value) in [("side",json!("ASK")),("availability_window",json!("SPOT")),("inventory_item_id",json!(item())),("owner_user_id",json!(seller())),("created_by_actor_user_id",json!(seller())),("provenance",json!("REAL")),("certifications",json!([])),("expires_at",Value::Null)] { assert_eq!(row[field],value,"{field}"); }
    assert_eq!(row["carbon_intensity_gco2_mj"],json!(0));
    assert_eq!(f.counts().await,json!({"orders":1,"trades":0,"audit":1,"events":1,"notifications":0,"receipts":0}));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_is_idempotent_per_item() {
    let mut f=Fixture::new(0,200).await;
    let first=f.publish(Some("original")).await; assert_eq!(first.0,StatusCode::OK,"{first:?}");
    let before=f.counts().await;
    f.sql("UPDATE inventory_items SET price_per_mt_usd=NULL,current_stock_mt=0,product_name='unmapped'").await;
    assert_eq!(f.publish(None).await,first);
    assert_eq!(f.publish(Some("reuse")).await,first);
    assert_eq!(f.counts().await,before);
    let reused:i64=sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE idempotency_key='reuse'").fetch_one(&mut f.observer).await.unwrap(); assert_eq!(reused,0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn keyed_publish_replays_original_identity_after_full_fill() {
    let mut f=Fixture::new(1,200).await;
    let first=f.publish(Some("filled")).await; assert_eq!(first.0,StatusCode::OK,"{first:?}");
    let before=f.counts().await;
    assert_eq!(f.publish(Some("filled")).await,first);
    assert_eq!(f.publish(None).await.0,StatusCode::BAD_REQUEST);
    assert_eq!(f.counts().await,before); assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_replays_before_role_and_kyc() {
    let mut f=Fixture::new(1,200).await;
    let first=f.publish(Some("principal")).await; assert_eq!(first.0,StatusCode::OK,"{first:?}");
    f.sql("UPDATE users SET role='BUYER',kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    let before=f.counts().await;
    assert_eq!(f.publish(Some("principal")).await,first);
    assert_eq!(f.publish(None).await.0,StatusCode::FORBIDDEN);
    f.sql("SELECT matching_fixture_member('00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002002','SUPPLIER')").await;
    let other=send(&f.router,Some(Uuid::from_u128(0x2002)),item(),Some("principal")).await;
    assert_eq!(other.0,StatusCode::FORBIDDEN); assert_eq!(other.1["detail"]["code"],"REPLAY_PRINCIPAL_MISMATCH");
    assert_eq!(send(&f.router,Some(seller()),Uuid::new_v4(),Some("principal")).await.0,StatusCode::CONFLICT);
    assert_eq!(f.counts().await,before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_requires_locked_supplier_eligibility() {
    for change in ["role_admin","pending","email_unverified","password_change","organization_pending","kyc_rejected","kyc_other_organization","moved"] {
        let mut f=Fixture::new(0,200).await;
        sqlx::query("SELECT matching_fixture_party($1,$2)").bind(seller()).bind(change).execute(&mut f.admin).await.unwrap();
        let before=f.counts().await; let reply=f.publish(None).await;
        assert_eq!(reply.0,StatusCode::FORBIDDEN,"{change}: {reply:?}");
        assert_eq!(f.counts().await,before); assert_eq!(f.stock().await,(Decimal::from(200),Decimal::ZERO));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_authentication_and_organization_precede_lookup() {
    let mut f=Fixture::new(0,200).await;
    assert_eq!(send(&f.router,None,Uuid::new_v4(),None).await.0,StatusCode::UNAUTHORIZED);
    f.sql("UPDATE users SET organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'").await;
    assert_eq!(f.publish(None).await.0,StatusCode::BAD_REQUEST);
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    assert_eq!(send(&f.router,Some(seller()),Uuid::new_v4(),None).await.0,StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_kyc_gate_and_grace_use_shared_policy() {
    let mut f=Fixture::new(0,200).await;
    f.sql("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'").await;
    let reply=f.publish(None).await; assert_eq!(reply.0,StatusCode::FORBIDDEN);
    let detail=reply.1["detail"].as_object().unwrap(); assert_eq!(detail.len(),4); assert_eq!(detail["code"],"KYC_REQUIRED"); assert_eq!(detail["kyc_status"],"PENDING"); assert!(detail["grace_expires_at"].is_null());
    let settings=CreateSettings{kyc_execution_gate_enabled:false,..Default::default()};
    assert_eq!(send(&component(&f.app,settings),Some(seller()),item(),None).await.0,StatusCode::OK);
    f.sql("UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    let settings=CreateSettings{kyc_execution_gate_enabled:false,..Default::default()};
    assert_eq!(send(&component(&f.app,settings),Some(seller()),item(),None).await.0,StatusCode::FORBIDDEN);
    let mut f=Fixture::new(0,200).await;
    f.sql("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001'; INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002000','PENDING',now(),now()+interval '1 hour','fixture')").await;
    assert_eq!(f.publish(None).await.0,StatusCode::OK);
    f.sql("UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    assert_eq!(f.publish(None).await.0,StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_header_uses_the_normalized_length() {
    let mut f=Fixture::new(0,200).await;
    for key in [" ".to_owned(),"a".repeat(256)] { assert_eq!(f.publish(Some(&key)).await.0,StatusCode::BAD_REQUEST); }
    let key=format!("  {}  ","a".repeat(255)); assert_eq!(f.publish(Some(&key)).await.0,StatusCode::OK);
    let before=f.counts().await; assert_eq!(f.publish(Some("")).await.0,StatusCode::OK); assert_eq!(f.counts().await,before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_validation_has_no_partial_effect() {
    for (sql,status) in [
      ("UPDATE inventory_items SET price_per_mt_usd=NULL",400),
      ("UPDATE inventory_items SET product_name='unmapped'",400),
      ("UPDATE ports SET name='not canonical' WHERE id='pub-port'",400),
      ("UPDATE inventory_items SET current_stock_mt=0",400),
      ("UPDATE inventory_items SET current_stock_mt=199",400),
      ("UPDATE products SET min_lot_size=0 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673'",409),
      ("UPDATE inventory_items SET msds_available=false",400),
      ("UPDATE inventory_items SET carbon_intensity_gco2_mj=NULL",400),
    ] {
        let mut f=Fixture::new(0,200).await; f.sql(sql).await;
        let before=f.counts().await; let reply=f.publish(None).await;
        assert_eq!(reply.0.as_u16(),status,"{sql}: {reply:?}"); assert_eq!(f.counts().await,before);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_refuses_bio_lng() {
    let mut f=Fixture::new(0,200).await;
    let product=f.app.policy.products.iter().find(|p|p.qualification_profile=="lng_v1").unwrap();
    sqlx::query("UPDATE inventory_items SET product_name=$1").bind(&product.name).execute(&mut f.admin).await.unwrap();
    let reply=f.publish(None).await; assert_eq!(reply.0,StatusCode::BAD_REQUEST,"{reply:?}"); assert!(reply.1["detail"]["message"].as_str().unwrap().contains("lng_terms"));
    assert_eq!(f.stock().await,(Decimal::from(200),Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_matching_can_be_disabled() {
    let mut f=Fixture::new(1,200).await;
    let settings=CreateSettings{auto_matching_enabled:false,..Default::default()};
    let reply=send(&component(&f.app,settings),Some(seller()),item(),None).await; assert_eq!(reply.0,StatusCode::OK,"{reply:?}");
    assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::from(200))); assert_eq!(f.counts().await["trades"],0);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_bound_preserves_atomicity() {
    for (n,stock,status,trades) in [(0,200,200,0),(1,200,200,1),(100,20000,200,100),(101,20000,200,100),(101,20200,409,0)] {
        let mut f=Fixture::new(n,stock).await; let before=f.counts().await; let reply=f.publish(None).await;
        assert_eq!(reply.0.as_u16(),status,"n={n},stock={stock}: {reply:?}"); assert_eq!(f.counts().await["trades"],trades);
        if status==409 { assert_eq!(f.counts().await,before); assert_eq!(f.stock().await,(Decimal::from(stock),Decimal::ZERO)); }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_expiry_releases_stock_with_separate_effects() {
    let mut f=Fixture::new(0,200).await;
    f.sql("SELECT publish_fixture_expired(1,true)").await;
    let reply=f.publish(None).await; assert_eq!(reply.0,StatusCode::OK,"{reply:?}");
    assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::from(400)));
    let expired:i64=sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE status::text='EXPIRED' AND version=2").fetch_one(&mut f.observer).await.unwrap(); assert_eq!(expired,1);
    assert_eq!(f.counts().await,json!({"orders":2,"trades":0,"audit":2,"events":2,"notifications":0,"receipts":0}));
    let mut f=Fixture::new(0,200).await; f.sql("SELECT publish_fixture_expired(1,false)").await;
    assert_eq!(f.publish(None).await.0,StatusCode::OK); assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::from(200)));
    let current:Decimal=sqlx::query_scalar("SELECT current_stock_mt FROM inventory_items WHERE id='00000000-0000-0000-0000-000000004001'").fetch_one(&mut f.observer).await.unwrap(); assert_eq!(current,Decimal::from(200));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publish_refuses_expired_backlog() {
    let mut f=Fixture::new(0,200).await; f.sql("SELECT publish_fixture_expired(101,true)").await;
    let before=f.counts().await; let stock=f.stock().await;
    let reply=f.publish(None).await; assert_eq!(reply.0,StatusCode::CONFLICT,"{reply:?}"); assert_eq!(f.counts().await,before); assert_eq!(f.stock().await,stock);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expiry_inconsistent_reservation_rolls_back_publication() {
    let mut f=Fixture::new(0,200).await; f.sql("SELECT publish_fixture_expired(1,true); UPDATE inventory_items SET reserved_stock_mt=0").await;
    let before=f.counts().await; assert_eq!(f.publish(None).await.0,StatusCode::CONFLICT); assert_eq!(f.counts().await,before); assert_eq!(f.stock().await,(Decimal::from(200),Decimal::ZERO));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_effect_failure_rolls_back_every_write() {
    let mut f=Fixture::new(1,200).await;
    f.sql("CREATE TRIGGER publish_fixture_refuse_effect BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION publish_fixture_refuse_effect()").await;
    let before=f.counts().await; let reply=f.publish(Some("rollback")).await;
    f.sql("DROP TRIGGER publish_fixture_refuse_effect ON audit_logs").await;
    assert!(!reply.0.is_success(),"{reply:?}"); assert_eq!(f.counts().await,before); assert_eq!(f.stock().await,(Decimal::from(200),Decimal::ZERO));
}

async fn concurrent(n:i32,stock:i64,key:Option<&str>) {
    let mut f=Fixture::new(n,stock).await;
    let mut blocker=f.pool.begin().await.unwrap();
    let product=f.app.policy.products.iter().find(|p|p.name=="B30").map(|p|p.id).unwrap_or_else(||f.app.policy.products.iter().find(|p|p.specification_standard.as_deref()==Some("ISO 8217:2024 RF 380")).unwrap().id);
    let point=f.app.policy.points.iter().find(|p|p.name=="Rotterdam").unwrap().id;
    let lock=verdaxis_read::order_write::market_slice_lock_key(product,Some(point),"SPOT");
    verdaxis_read::routes::market_write::order_create::contract_hold_key(&mut blocker,lock).await;
    let router=f.router.clone(); let owned_key=key.map(str::to_owned);
    let first=tokio::spawn(async move{send(&router,Some(seller()),item(),owned_key.as_deref()).await});
    tokio::time::sleep(Duration::from_millis(50)).await;
    let router=f.router.clone();let owned_key=key.map(str::to_owned);
    let second=tokio::spawn(async move{send(&router,Some(seller()),item(),owned_key.as_deref()).await});
    blocker.commit().await.unwrap();
    let a=first.await.unwrap(); let b=second.await.unwrap();
    assert!(a.0.is_success()||b.0.is_success(),"{a:?} {b:?}");
    for response in [&a,&b] { assert!([200,400,409,503].contains(&response.0.as_u16()),"{response:?}"); }
    let again=f.publish(key).await;
    if n==1&&key.is_none() { assert_eq!(again.0,StatusCode::BAD_REQUEST); } else { assert_eq!(again.0,StatusCode::OK,"{again:?}"); }
    let counts=f.counts().await; assert_eq!(counts["orders"],n+1); assert_eq!(counts["trades"],n);
    assert_eq!(counts["audit"],n+1); assert_eq!(counts["events"],if n==0{1}else{3});
    assert_eq!(f.stock().await,(Decimal::ZERO,Decimal::from(stock-n as i64*200)));
}
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn double_unkeyed_publish_with_live_remainder_returns_one_listing(){concurrent(0,200,None).await;concurrent(1,400,None).await;}
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn double_unkeyed_publish_after_full_fill_refuses_without_effect(){concurrent(1,200,None).await;}
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_keyed_publication_replays_original_identity(){concurrent(1,200,Some("same-key")).await;}

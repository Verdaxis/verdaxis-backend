#![cfg(feature = "contract-tests")]
use axum::{Router, body::Body, http::{Request, StatusCode}};
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
fn id(n: u128) -> Uuid { Uuid::from_u128(n) }
struct Fixture { admin: PgConnection, pool: PgPool, router: Router, trade: Uuid }
impl Fixture {
    async fn new(initiated: &str, legacy: bool) -> Self {
        let a = std::env::var("RUST_CONFIRM_ADMIN_DATABASE_URL").expect("disposable admin URL");
        let r = std::env::var("RUST_CONFIRM_DATABASE_URL").expect("disposable runtime URL");
        let au = url::Url::parse(&a).unwrap(); let ru = url::Url::parse(&r).unwrap();
        assert_eq!(au.host_str(), Some("127.0.0.1")); assert!(au.path().ends_with("_test"));
        assert_eq!((au.host_str(), au.port(), au.path()), (ru.host_str(), ru.port(), ru.path()));
        assert_ne!(au.username(), ru.username());
        let mut admin = PgConnection::connect(&a).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase9_trades.sql")).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase9_confirm.sql")).execute(&mut admin).await.unwrap();
        let policy: Value = serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let p = policy["products"].as_array().unwrap().iter().find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK" && p.get("specification_standard").is_none()).unwrap();
        sqlx::query("SELECT trades_fixture_seed($1,$2)").bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap()).bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap()).execute(&mut admin).await.unwrap();
        let trade = sqlx::query_scalar("SELECT confirm_fixture_pending($1,$2)").bind(initiated).bind(legacy).fetch_one(&mut admin).await.unwrap();
        let pool = PgPoolOptions::new().max_connections(8).connect(&r).await.unwrap();
        let router = contract_router(pool.clone(), KEY, true);
        Self { admin, pool, router, trade }
    }
    async fn call(&self, action: &str, actor: u128, key: Option<&str>) -> (StatusCode,Value) {
        let headers: Vec<_> = key.map(|key| vec![("idempotency-key",key)]).unwrap_or_default();
        send(&self.router, &format!("/api/trades/{}/{action}", self.trade), Some(actor), &headers).await
    }
    async fn snapshot(&mut self) -> Value { sqlx::query_scalar("SELECT confirm_fixture_snapshot()").fetch_one(&mut self.admin).await.unwrap() }
    async fn sql(&mut self, query: &str) { sqlx::raw_sql(sqlx::AssertSqlSafe(query.to_owned())).execute(&mut self.admin).await.unwrap(); }
    async fn change(&mut self, actor: u128, change: &str) { sqlx::query("SELECT trades_fixture_change($1,$2)").bind(id(actor)).bind(change).execute(&mut self.admin).await.unwrap(); }
    async fn stock(&mut self) -> (String,String) { sqlx::query_as("SELECT current_stock_mt::text,reserved_stock_mt::text FROM inventory_items").fetch_one(&mut self.admin).await.unwrap() }
    async fn order(&mut self) -> (String,String,i32) { sqlx::query_as("SELECT status::text,remaining_quantity_mt::text,version FROM orderbook_orders").fetch_one(&mut self.admin).await.unwrap() }
}
async fn send(router: &Router, path: &str, actor: Option<u128>, headers: &[(&str,&str)]) -> (StatusCode,Value) {
    let mut request = Request::builder().method("PUT").uri(path);
    if let Some(actor) = actor {
        let now = chrono::Utc::now().timestamp();
        let token = encode(&Header::default(), &json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":"lifecycle-tests","aud":"lifecycle-tests"}), &EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (k,v) in headers { request = request.header(*k,*v); }
    let response = router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status(); let bytes = axum::body::to_bytes(response.into_body(), 1024*1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
fn amount(text: &str) -> rust_decimal::Decimal { text.parse().unwrap() }

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_replay_adds_no_second_activation() {
    let mut f = Fixture::new("BUYER",false).await;
    f.sql("SELECT confirm_fixture_referrals()").await;
    let (status,body) = f.call("confirm",SELLER,Some("confirmation")).await;
    assert_eq!(status,StatusCode::OK,"{body}"); assert_eq!(body["status"],"CONFIRMED");
    let referrals: (i64,i64) = sqlx::query_as("SELECT count(*),count(activated_at) FROM referrals WHERE status='ACTIVE'").fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(referrals,(2,2));
    let after = f.snapshot().await;
    f.change(SELLER,"kyc").await;
    let (status,replay) = f.call("confirm",SELLER,Some("confirmation")).await;
    assert_eq!(status,StatusCode::OK,"{replay}"); assert_eq!(replay,body); assert_eq!(f.snapshot().await,after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_requires_the_counterparty() {
    for (initiated,allowed,denied) in [("BUYER",SELLER,vec![BUYER,MEMBER,STRANGER]),("SELLER",BUYER,vec![SELLER,MEMBER,STRANGER])] {
        let mut f = Fixture::new(initiated,false).await;
        for actor in denied { let before=f.snapshot().await; let (s,b)=f.call("confirm",actor,None).await; assert_eq!(s,StatusCode::FORBIDDEN,"{initiated} actor={actor}: {b}"); assert_eq!(f.snapshot().await,before); }
        let (s,b)=f.call("confirm",allowed,None).await; assert_eq!(s,StatusCode::OK,"{b}");
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_consumes_reserved_stock_exactly() {
    let mut f=Fixture::new("BUYER",false).await; let order=f.order().await;
    let (s,b)=f.call("confirm",SELLER,None).await; assert_eq!(s,StatusCode::OK,"{b}");
    let (current,reserved)=f.stock().await; assert_eq!(amount(&current),amount("0")); assert_eq!(amount(&reserved),amount("7")); assert_eq!(f.order().await,order);
    assert_eq!(b["buyer_id"],id(0x1000).to_string()); assert_eq!(b["seller_id"],id(0x2000).to_string()); assert_eq!(b["commission_fee_per_mt_usd"],"2.00");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_of_a_live_order_restores_the_remainder() {
    let mut f=Fixture::new("BUYER",false).await; let version=f.order().await.2;
    let (s,b)=f.call("decline",SELLER,None).await; assert_eq!(s,StatusCode::OK,"{b}"); assert_eq!(b["status"],"DECLINED");
    let (status,remainder,next)=f.order().await; assert_eq!(status,"OPEN"); assert_eq!(amount(&remainder),amount("10")); assert_eq!(next,version+1);
    assert_eq!(amount(&f.stock().await.1),amount("10"));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_of_a_withdrawn_order_releases_without_reviving() {
    for status in ["CANCELLED","EXPIRED"] {
        let mut f=Fixture::new("BUYER",false).await;
        f.sql(&format!("UPDATE orderbook_orders SET status='{status}'; UPDATE inventory_items SET current_stock_mt=7,reserved_stock_mt=3")).await;
        let order=f.order().await;
        let (s,b)=f.call("decline",SELLER,None).await; assert_eq!(s,StatusCode::OK,"{b}"); assert_eq!(f.order().await,order);
        let (current,reserved)=f.stock().await; assert_eq!(amount(&current),amount("10")); assert_eq!(amount(&reserved),amount("0"));
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action='order.expired'").fetch_one(&mut f.admin).await.unwrap(); assert_eq!(count,0);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn elapsed_decline_releases_the_full_restored_balance() {
    let mut f=Fixture::new("BUYER",false).await; f.change(1,"expired").await;
    let (s,b)=f.call("decline",SELLER,Some("elapsed")).await; assert_eq!(s,StatusCode::OK,"{b}");
    assert_eq!(f.order().await.0,"EXPIRED"); assert_eq!(amount(&f.order().await.1),amount("10"));
    let (current,reserved)=f.stock().await; assert_eq!(amount(&current),amount("10")); assert_eq!(amount(&reserved),amount("0"));
    let audit:Value=sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='order.expired'").fetch_one(&mut f.admin).await.unwrap(); assert_eq!(audit["status"],"EXPIRED");
    let after=f.snapshot().await; assert_eq!(f.call("decline",SELLER,Some("elapsed")).await.0,StatusCode::OK); assert_eq!(f.snapshot().await,after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn cleanup_uses_the_recorded_party_and_legacy_membership_only_when_null() {
    for change in ["rejected","detached","moved","kyc","email","password"] {
        let mut f=Fixture::new("BUYER",false).await; f.change(SELLER,change).await;
        let (s,b)=f.call("decline",SELLER,None).await; assert_eq!(s,StatusCode::OK,"{change}: {b}"); assert!(!b["seller_id"].is_null());
    }
    for initiated in ["BUYER","SELLER"] {
        let f=Fixture::new(initiated,true).await;
        let actor=if initiated=="BUYER" {SELLER} else {MEMBER};
        let (s,b)=f.call("decline",actor,None).await; assert_eq!(s,StatusCode::OK,"{b}");
    }
    let mut f=Fixture::new("SELLER",false).await; let before=f.snapshot().await;
    assert_eq!(f.call("decline",MEMBER,None).await.0,StatusCode::FORBIDDEN); assert_eq!(f.snapshot().await,before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_fresh_admission_and_both_recorded_parties_are_rechecked() {
    for actor in [BUYER,SELLER] { for change in ["rejected","pending","detached","moved","kyc","email","password"] {
        let mut f=Fixture::new("BUYER",false).await; f.change(actor,change).await; let before=f.snapshot().await;
        let (s,b)=f.call("confirm",SELLER,None).await;
        assert_eq!(s,if actor==SELLER {StatusCode::FORBIDDEN} else {StatusCode::CONFLICT},"actor={actor} {change}: {b}"); assert_eq!(f.snapshot().await,before);
    }}
    let mut f=Fixture::new("BUYER",false).await; f.change(SELLER,"kyc").await;
    let (_,b)=f.call("confirm",SELLER,None).await; assert_eq!(b["detail"]["code"],"KYC_REQUIRED"); assert!(b["detail"].get("kyc_status").is_some()); assert!(b["detail"].get("grace_expires_at").is_some());
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn support_admin_cannot_settle() {
    for action in ["confirm","decline"] {
        let mut f=Fixture::new("BUYER",false).await; let before=f.snapshot().await;
        let (s,b)=f.call(action,ADMIN,None).await; assert_eq!(s,StatusCode::FORBIDDEN,"{b}"); assert_eq!(f.snapshot().await,before);
        for header in ["not-a-uuid","00000000-0000-0000-0000-000000006000"] {
            let (s,b)=send(&f.router,&format!("/api/trades/{}/{action}",f.trade),Some(SELLER),&[("x-verdaxis-market-support-context",header)]).await;
            assert_eq!(s,StatusCode::FORBIDDEN); assert_eq!(b["detail"]["code"],"MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_and_decline_are_receipted() {
    for action in ["confirm","decline"] {
        let mut f=Fixture::new("BUYER",false).await;
        let (s,b)=f.call(action,SELLER,Some("  same-key  ")).await; assert_eq!(s,StatusCode::OK,"{b}"); let after=f.snapshot().await;
        let (s,replay)=f.call(action,SELLER,Some("same-key")).await; assert_eq!(s,StatusCode::OK,"{replay}"); assert_eq!(b,replay); assert_eq!(f.snapshot().await,after);
        assert_eq!(f.call(action,SELLER,None).await.0,StatusCode::BAD_REQUEST);
        let (s,_)=send(&f.router,&format!("/api/trades/{}/{action}",Uuid::new_v4()),Some(SELLER),&[("idempotency-key","same-key")]).await; assert_eq!(s,StatusCode::CONFLICT);
        assert_eq!(f.snapshot().await,after);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn key_limits_authentication_and_path_validation_have_no_effects() {
    let mut f=Fixture::new("BUYER",false).await; let before=f.snapshot().await;
    for key in [" ".to_owned(),"a".repeat(256)] { assert_eq!(f.call("confirm",SELLER,Some(&key)).await.0,StatusCode::BAD_REQUEST); }
    assert_eq!(send(&f.router,"/api/trades/not-a-uuid/confirm",None,&[]).await.0,StatusCode::UNAUTHORIZED);
    assert_eq!(send(&f.router,"/api/trades/not-a-uuid/confirm",Some(SELLER),&[]).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(f.snapshot().await,before);
    let (s,b)=send(&f.router,&format!("/api/trades/{}/confirm",f.trade),Some(SELLER),&[("if-match","garbage")]).await; assert_eq!(s,StatusCode::OK,"{b}");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn short_reservation_and_failed_effects_roll_back_the_whole_transition() {
    for action in ["confirm","decline"] {
        let mut f=Fixture::new("BUYER",false).await;
        f.sql("UPDATE orderbook_orders SET status='CANCELLED'; UPDATE inventory_items SET reserved_stock_mt=2,current_stock_mt=7").await;
        let before=f.snapshot().await; let (s,b)=f.call(action,SELLER,Some("short")).await;
        assert_eq!(s,StatusCode::CONFLICT,"{b}"); assert_eq!(f.snapshot().await,before);
        let mut f=Fixture::new("BUYER",false).await; f.sql("SELECT trades_fixture_reject_audit()").await; let before=f.snapshot().await;
        assert!(!f.call(action,SELLER,Some("audit")).await.0.is_success()); assert_eq!(f.snapshot().await,before); f.sql("SELECT trades_fixture_allow_audit()").await;
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn lifecycle_effects_have_one_scoped_envelope_and_current_initiator_recipients() {
    for action in ["confirm","decline"] {
        let mut f=Fixture::new("BUYER",false).await;
        let (s,b)=send(&f.router,&format!("/api/trades/{}/{action}",f.trade),Some(SELLER),&[("x-forwarded-for","203.0.113.12"),("x-request-id",&"r".repeat(100))]).await; assert_eq!(s,StatusCode::OK,"{b}");
        let audit=sqlx::query("SELECT action,resource_type,resource_id,changes,ip_address,request_id FROM audit_logs WHERE resource_type='trade'").fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(audit.get::<String,_>("action"),format!("trade.{}",if action=="confirm" {"confirmed"} else {"declined"})); assert_eq!(audit.get::<String,_>("resource_id"),f.trade.to_string()); assert_eq!(audit.get::<Value,_>("changes"),json!({"status":b["status"]})); assert_ne!(audit.get::<Option<String>,_>("ip_address").as_deref(),Some("203.0.113.12")); assert!(audit.get::<Option<String>,_>("request_id").unwrap().len()<=36);
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE type='ORDER_UPDATE' AND recipient_id IN ('00000000-0000-0000-0000-000000001001','00000000-0000-0000-0000-000000001002')").fetch_one(&mut f.admin).await.unwrap(); assert_eq!(count,2);
        let rows=sqlx::query("SELECT event_type,aggregate_type,aggregate_id,participant_org_ids,payload FROM market_event_outbox").fetch_all(&mut f.admin).await.unwrap(); assert_eq!(rows.len(),1);
        let row=&rows[0]; assert_eq!(row.get::<String,_>("aggregate_type"),"trade"); assert_eq!(row.get::<String,_>("aggregate_id"),f.trade.to_string());
        let scope:Value=row.get("participant_org_ids"); assert_eq!(scope,json!([id(0x1000),id(0x2000)]));
        let payload:Value=row.get("payload"); assert_eq!(payload["schema_version"],1); assert_eq!(payload["id"],f.trade.to_string()); assert_eq!(payload["status"],b["status"]); assert!(payload.get("commission_fee_per_mt_usd").is_none());
        assert_eq!(payload.get("quantity").is_some(),action=="confirm"); assert_eq!(payload.get("price").is_some(),action=="confirm");
    }
}

async fn wait_for_lock(admin: &mut PgConnection) {
    for _ in 0..200 { let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid())").fetch_one(&mut *admin).await.unwrap(); if waiting {return;} tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
    panic!("the independent command did not wait on a PostgreSQL lock");
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn confirm_and_decline_have_one_winner() {
    for first in ["confirm","decline"] {
        let mut f=Fixture::new("BUYER",false).await;
        let mut holder=f.pool.begin().await.unwrap(); sqlx::query("SELECT id FROM organizations WHERE id='00000000-0000-0000-0000-000000001000' FOR UPDATE").execute(&mut *holder).await.unwrap();
        let r=f.router.clone(); let path=format!("/api/trades/{}/{first}",f.trade);
        let a=tokio::spawn(async move {send(&r,&path,Some(SELLER),&[]).await}); wait_for_lock(&mut f.admin).await; holder.commit().await.unwrap();
        let (s,b)=a.await.unwrap(); assert_eq!(s,StatusCode::OK,"{b}");
        let second=if first=="confirm" {"decline"} else {"confirm"}; assert_eq!(f.call(second,SELLER,None).await.0,StatusCode::BAD_REQUEST);
        assert_eq!(amount(&f.stock().await.1),amount(if first=="confirm" {"7"} else {"10"}));
    }
    let mut f=Fixture::new("BUYER",false).await;
    let (a,b)=tokio::join!(f.call("confirm",SELLER,None),f.call("decline",SELLER,None));
    assert_eq!(usize::from(a.0==StatusCode::OK)+usize::from(b.0==StatusCode::OK),1,"{a:?} {b:?}");
    assert!([StatusCode::BAD_REQUEST,StatusCode::CONFLICT].contains(&if a.0==StatusCode::OK {b.0} else {a.0}));
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE resource_type='trade'").fetch_one(&mut f.admin).await.unwrap(); assert_eq!(count,1);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn decline_clock_is_captured_after_the_order_lock_wait() {
    let mut f=Fixture::new("BUYER",false).await;
    f.sql("UPDATE orderbook_orders SET expires_at=clock_timestamp()+interval '250 milliseconds'").await;
    let mut holder=f.pool.begin().await.unwrap(); sqlx::query("SELECT id FROM orderbook_orders FOR UPDATE").execute(&mut *holder).await.unwrap();
    let router=f.router.clone(); let path=format!("/api/trades/{}/decline",f.trade);
    let command=tokio::spawn(async move {send(&router,&path,Some(SELLER),&[]).await}); wait_for_lock(&mut f.admin).await;
    tokio::time::sleep(std::time::Duration::from_millis(275)).await; holder.commit().await.unwrap();
    let (s,b)=command.await.unwrap(); assert_eq!(s,StatusCode::OK,"{b}"); assert_eq!(f.order().await.0,"EXPIRED"); assert_eq!(amount(&f.stock().await.1),amount("0"));
}

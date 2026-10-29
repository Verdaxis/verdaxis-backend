#![cfg(feature = "contract-tests")]
use axum::{Router, body::Body, http::{Request, StatusCode}};
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
fn id(n:u128)->Uuid { Uuid::from_u128(n) }
fn delivery()->Value { json!({"final_quantity_mt":"3","final_price_per_mt":"100"}) }
fn amount(v:&Value)->rust_decimal::Decimal { v.as_str().unwrap().parse().unwrap() }
struct Fixture { admin:PgConnection, pool:PgPool, router:Router, trade:Uuid }
impl Fixture {
    async fn new()->Self {
        let a=std::env::var("RUST_SETTLEMENT_ADMIN_DATABASE_URL").expect("disposable admin URL");
        let r=std::env::var("RUST_SETTLEMENT_DATABASE_URL").expect("disposable runtime URL");
        let au=url::Url::parse(&a).unwrap(); let ru=url::Url::parse(&r).unwrap();
        assert_eq!(au.host_str(),Some("127.0.0.1")); assert!(au.path().ends_with("_test"));
        assert_eq!((au.host_str(),au.port(),au.path()),(ru.host_str(),ru.port(),ru.path()));
        assert_ne!(au.username(),ru.username());
        let mut admin=PgConnection::connect(&a).await.unwrap();
        for sql in [include_str!("fixtures/phase9_trades.sql"),include_str!("fixtures/phase9_confirm.sql"),include_str!("fixtures/phase9_settlement.sql")] { sqlx::raw_sql(sql).execute(&mut admin).await.unwrap(); }
        let policy:Value=serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let p=policy["products"].as_array().unwrap().iter().find(|p|p["qualification_profile"]=="scalar"&&p["execution_mode"]=="ORDERBOOK"&&p.get("specification_standard").is_none()).unwrap();
        sqlx::query("SELECT trades_fixture_seed($1,$2)").bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap()).bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap()).execute(&mut admin).await.unwrap();
        sqlx::query("SELECT set_config('confirm.policy',$1,false)").bind(policy["products"].to_string()).execute(&mut admin).await.unwrap();
        sqlx::query("SELECT confirm_fixture_pending('BUYER',false)").execute(&mut admin).await.unwrap();
        let trade=sqlx::query_scalar("SELECT settlement_fixture_confirmed()").fetch_one(&mut admin).await.unwrap();
        let pool=PgPoolOptions::new().max_connections(8).connect(&r).await.unwrap();
        let router=contract_router(pool.clone(),KEY,true);
        Self{admin,pool,router,trade}
    }
    async fn sql(&mut self,sql:&str) { sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned())).execute(&mut self.admin).await.unwrap(); }
    async fn change(&mut self,actor:u128,change:&str) { sqlx::query("SELECT trades_fixture_change($1,$2)").bind(id(actor)).bind(change).execute(&mut self.admin).await.unwrap(); }
    async fn snapshot(&mut self)->Value { sqlx::query_scalar("SELECT confirm_fixture_snapshot()").fetch_one(&mut self.admin).await.unwrap() }
    async fn call(&self,action:&str,actor:u128,key:Option<&str>,body:Value)->(StatusCode,Value) {
        let headers=key.map(|k|vec![("idempotency-key",k)]).unwrap_or_default();
        send(&self.router,self.trade,action,Some(actor),&headers,body.to_string(),0).await
    }
    async fn deliver(&self,actor:u128,key:Option<&str>)->(StatusCode,Value) { self.call("deliver",actor,key,delivery()).await }
    async fn pay(&self,actor:u128,key:Option<&str>)->(StatusCode,Value) { self.call("pay",actor,key,Value::Null).await }
    async fn assisted(&mut self) {
        self.sql("SELECT confirm_fixture_assisted(); SELECT confirm_fixture_pending('BUYER',false); SELECT settlement_fixture_confirmed()").await;
        self.trade=sqlx::query_scalar("SELECT id FROM trades").fetch_one(&mut self.admin).await.unwrap();
    }
}
async fn send(router:&Router,trade:Uuid,action:&str,actor:Option<u128>,headers:&[(&str,&str)],body:String,issued_offset:i64)->(StatusCode,Value) {
    let mut request=Request::builder().method(if action=="pay"{"POST"}else{"PUT"}).uri(format!("/api/trades/{trade}/{action}"));
    if let Some(actor)=actor {
        let now=chrono::Utc::now().timestamp();
        let token=encode(&Header::default(),&json!({"sub":id(actor),"type":"access","iat":now+issued_offset,"exp":if issued_offset < -1000 {now-1}else{now+600},"iss":"lifecycle-tests","aud":"lifecycle-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        request=request.header("authorization",format!("Bearer {token}"));
    }
    for (k,v) in headers {request=request.header(*k,*v);}
    let response=router.clone().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap();
    let status=response.status(); let bytes=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
    (status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
async fn ok(result:(StatusCode,Value),status:&str)->Value { assert_eq!(result.0,StatusCode::OK,"{}",result.1); assert_eq!(result.1["status"],status); result.1 }

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn detached_party_delivers_and_pays_its_preexisting_trade() {
    let mut f=Fixture::new().await; f.change(SELLER,"detach").await;
    let original=f.snapshot().await;
    let delivered=ok(f.deliver(SELLER,Some("delivery")).await,"DELIVERED").await;
    assert_eq!(amount(&delivered["final_total_usd"]),"300".parse().unwrap());
    assert_eq!(amount(&delivered["commission_amount_usd"]),"6".parse().unwrap());
    ok(f.pay(SELLER,Some("payment")).await,"PAID").await;
    let after=f.snapshot().await;
    for field in ["orders","inventory","referrals","benchmarks","targets"] {assert_eq!(after[field],original[field],"{field}");}
    assert_eq!(after["audit"],2); assert_eq!(after["outbox"],2); assert_eq!(after["receipts"],2);
    let receipts:Vec<(Uuid,Option<Uuid>)>=sqlx::query_as("SELECT effective_organization_id,support_context_id FROM market_command_results").fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(receipts,vec![(id(0x2000),None);2]);
    assert_eq!(ok(f.deliver(SELLER,Some("delivery")).await,"DELIVERED").await,delivered);
    assert_eq!(f.snapshot().await,after);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn deliver_accepts_either_recorded_party_and_rejects_strangers() {
    for actor in [BUYER,SELLER] {
        let mut f=Fixture::new().await;
        for denied in [MEMBER,STRANGER,ADMIN] {
            let before=f.snapshot().await; let (s,b)=f.deliver(denied,None).await;
            assert_eq!(s,StatusCode::FORBIDDEN,"actor={denied}: {b}"); assert_eq!(f.snapshot().await,before);
        }
        ok(f.deliver(actor,None).await,"DELIVERED").await;
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn pay_requires_the_recorded_seller_and_supplier_role() {
    let mut f=Fixture::new().await; ok(f.deliver(BUYER,None).await,"DELIVERED").await;
    for actor in [BUYER,MEMBER,STRANGER,ADMIN] {
        let before=f.snapshot().await; let (s,b)=f.pay(actor,None).await;
        assert_eq!(s,StatusCode::FORBIDDEN,"{b}"); assert_eq!(f.snapshot().await,before);
    }
    f.sql("UPDATE users SET role='BUYER' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    assert_eq!(f.pay(SELLER,None).await.0,StatusCode::FORBIDDEN);
    f.sql("UPDATE users SET role='SUPPLIER' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    ok(f.pay(SELLER,None).await,"PAID").await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn keyed_and_unkeyed_cleanup_after_rejection_unlink_and_move() {
    for change in ["reject","detach","move","org_reject","kyc","email","password_required"] {
        for key in [None,Some("cleanup")] {
            let mut f=Fixture::new().await; f.change(SELLER,change).await;
            ok(f.deliver(SELLER,key).await,"DELIVERED").await; ok(f.pay(SELLER,key).await,"PAID").await;
        }
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn detached_party_replays_a_rust_receipt() {
    for change in ["reject","detach","move","org_reject"] {
        let mut f=Fixture::new().await;
        let delivered=ok(f.deliver(SELLER,Some("settle")).await,"DELIVERED").await;
        let paid=ok(f.pay(SELLER,Some("settle")).await,"PAID").await;
        f.change(SELLER,change).await; let before=f.snapshot().await;
        assert_eq!(ok(f.deliver(SELLER,Some(" settle ")).await,"DELIVERED").await,delivered);
        assert_eq!(ok(f.pay(SELLER,Some("settle")).await,"PAID").await,paid);
        assert_eq!(f.snapshot().await,before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn delivery_deviation_boundary_is_inclusive() {
    for (price,status) in [("90",200),("110",200),("90.01",200),("109.99",200),("89.99",400),("110.01",400)] {
        let mut f=Fixture::new().await; let before=f.snapshot().await;
        let (s,b)=f.call("deliver",BUYER,None,json!({"final_quantity_mt":"3","final_price_per_mt":price})).await;
        assert_eq!(s.as_u16(),status,"{price}: {b}"); if status!=200 {assert_eq!(f.snapshot().await,before);}
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn delivery_quantity_cannot_exceed_the_trade() {
    let mut f=Fixture::new().await; let before=f.snapshot().await;
    let (s,b)=f.call("deliver",BUYER,None,json!({"final_quantity_mt":"3.01","final_price_per_mt":"100"})).await;
    assert_eq!(s,StatusCode::BAD_REQUEST,"{b}"); assert_eq!(f.snapshot().await,before);
    ok(f.deliver(BUYER,None).await,"DELIVERED").await;
    assert_eq!(f.deliver(BUYER,None).await.0,StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn settlement_rounds_once_half_up() {
    for (fee,rate,total,commission) in [(Some("1.50"),"0","3.02","0.05"),(None,"16.550","3.02","0.50"),(Some("0"),"0","3.02","0")] {
        let mut f=Fixture::new().await;
        sqlx::query("SELECT settlement_fixture_confirmed(0.03,100.50,$1,$2)").bind(fee.map(|s|s.parse::<rust_decimal::Decimal>().unwrap())).bind(rate.parse::<rust_decimal::Decimal>().unwrap()).execute(&mut f.admin).await.unwrap();
        let b=ok(f.call("deliver",SELLER,None,json!({"final_quantity_mt":"0.03","final_price_per_mt":"100.50"})).await,"DELIVERED").await;
        assert_eq!(amount(&b["final_total_usd"]),total.parse().unwrap()); assert_eq!(amount(&b["commission_amount_usd"]),commission.parse().unwrap());
        let commissions:i64=sqlx::query_scalar("SELECT count(*) FROM commissions").fetch_one(&mut f.admin).await.unwrap(); assert_eq!(commissions,0);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn request_and_authentication_precedence_is_explicit() {
    let mut f=Fixture::new().await; let before=f.snapshot().await;
    for (actor,action,body,headers,expected) in [
        (None,"deliver","{",vec![],401),
        (Some(BUYER),"pay","{",vec![],403),
        (Some(BUYER),"deliver","{",vec![],422),
        (None,"deliver","{",vec![("x-verdaxis-market-support-context","not-a-context")],403),
        (Some(BUYER),"deliver","{",vec![("x-verdaxis-market-support-context"," ")],422),
    ] {
        let (s,b)=send(&f.router,f.trade,action,actor,&headers,body.into(),0).await;
        assert_eq!(s.as_u16(),expected,"{b}"); assert_eq!(f.snapshot().await,before);
    }
    for body in ["{}","null",r#"{"final_quantity_mt":null,"final_price_per_mt":100}"#,r#"{"final_quantity_mt":1e0,"final_price_per_mt":100}"#] {
        assert_eq!(send(&f.router,f.trade,"deliver",Some(BUYER),&[],body.into(),0).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(send(&f.router,f.trade,"deliver",Some(BUYER),&[]," ".repeat(65537),0).await.0,StatusCode::PAYLOAD_TOO_LARGE);
    for key in [" ".to_owned(),"x".repeat(256)] {assert_eq!(f.deliver(BUYER,Some(&key)).await.0,StatusCode::BAD_REQUEST);}
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn password_cutoff_always_applies_and_only_rejection_skips_revocation() {
    for (state,cutoff,expected) in [("REJECTED","password_changed_at",401),("APPROVED","authentication_revoked_at",401),("PENDING","authentication_revoked_at",401),("REJECTED","authentication_revoked_at",200)] {
        let mut f=Fixture::new().await;
        f.sql(&format!("UPDATE users SET status='{state}',{cutoff}=now()-interval '30 seconds' WHERE id='00000000-0000-0000-0000-000000002001'")).await;
        let before=f.snapshot().await;
        let (s,b)=send(&f.router,f.trade,"deliver",Some(SELLER),&[],delivery().to_string(),-60).await;
        assert_eq!(s.as_u16(),expected,"{state} {cutoff}: {b}"); if expected!=200 {assert_eq!(f.snapshot().await,before);}
    }
    let f=Fixture::new().await;
    assert_eq!(send(&f.router,f.trade,"deliver",Some(SELLER),&[],delivery().to_string(),-2000).await.0,StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn stranger_and_cross_trade_actor_are_refused() {
    let mut f=Fixture::new().await;
    ok(f.deliver(SELLER,Some("same")).await,"DELIVERED").await;
    let before=f.snapshot().await;
    assert_eq!(f.deliver(STRANGER,Some("same")).await.0,StatusCode::FORBIDDEN);
    assert_eq!(f.call("deliver",SELLER,Some("same"),json!({"final_quantity_mt":"2","final_price_per_mt":"100"})).await.0,StatusCode::CONFLICT);
    let other:Uuid=sqlx::query_scalar("SELECT settlement_fixture_clone()").fetch_one(&mut f.admin).await.unwrap();
    let before_other=f.snapshot().await;
    let (s,b)=send(&f.router,other,"deliver",Some(SELLER),&[("idempotency-key","same")],delivery().to_string(),0).await;
    assert_eq!(s,StatusCode::CONFLICT,"{b}"); assert_eq!(b["detail"]["code"],"RECEIPT_RESOURCE_MISMATCH");
    assert_eq!(f.snapshot().await,before_other);
    assert_eq!(send(&f.router,Uuid::new_v4(),"deliver",Some(SELLER),&[("idempotency-key","same")],delivery().to_string(),0).await.0,StatusCode::NOT_FOUND);
    assert_eq!(before["audit"],before_other["audit"]);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn old_python_receipt_replays_after_detachment_in_rust() {
    let mut f=Fixture::new().await;
    let body=json!({"status":"DELIVERED","original_response":true});
    // The persisted hash is the Python canonical Decimal/UUID payload.
    let payload=json!({"operation":"trade.deliver","trade_id":f.trade,"final_quantity_mt":"3","final_price_per_mt":"100"});
    use sha2::{Digest,Sha256};
    let mut entries=std::collections::BTreeMap::new(); for (k,v) in payload.as_object().unwrap() {entries.insert(k,v);}
    let hash=format!("{:x}",Sha256::digest(serde_json::to_vec(&entries).unwrap()));
    sqlx::query("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body) VALUES($1,$2,$3,'trade.deliver','old',$4,'trade',$5,200,$6)").bind(Uuid::new_v4()).bind(id(SELLER)).bind(id(0x2000)).bind(hash).bind(f.trade).bind(&body).execute(&mut f.admin).await.unwrap();
    f.change(SELLER,"detach").await; let before=f.snapshot().await;
    assert_eq!(ok(f.deliver(SELLER,Some("old")).await,"DELIVERED").await,body); assert_eq!(f.snapshot().await,before);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn assisted_customers_settle_but_staff_and_former_members_do_not() {
    let mut f=Fixture::new().await; f.assisted().await;
    assert_eq!(f.deliver(ADMIN,None).await.0,StatusCode::FORBIDDEN);
    ok(f.deliver(SELLER,Some("assisted")).await,"DELIVERED").await;
    f.change(SELLER,"detach").await; let before=f.snapshot().await;
    assert_eq!(f.deliver(SELLER,Some("assisted")).await.0,StatusCode::FORBIDDEN);
    assert_eq!(f.pay(SELLER,None).await.0,StatusCode::FORBIDDEN); assert_eq!(f.snapshot().await,before);
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    ok(f.pay(SELLER,None).await,"PAID").await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn settlement_effects_have_exact_scope_and_transition_time() {
    for actor in [BUYER,SELLER] {
        let mut f=Fixture::new().await;
        f.sql("UPDATE users SET status='REJECTED',email_verified=false WHERE id='00000000-0000-0000-0000-000000001002'; SELECT confirm_fixture_mirrors()").await;
        let before=f.snapshot().await;
        let delivered=ok(f.deliver(actor,Some("delivery")).await,"DELIVERED").await;
        let paid=ok(f.pay(SELLER,Some("pay")).await,"PAID").await;
        let after=f.snapshot().await;
        for field in ["orders","inventory","referrals","benchmarks","targets"] {assert_eq!(before[field],after[field],"{field}");}
        let rows:Vec<(String,Value,Vec<Uuid>)>=sqlx::query_as("SELECT event_type,payload,participant_org_ids FROM market_event_outbox ORDER BY created_at,id").fetch_all(&mut f.admin).await.unwrap();
        assert_eq!(rows.len(),2);
        for (kind,payload,orgs) in rows {
            assert_eq!(orgs,vec![id(0x1000),id(0x2000)]);
            assert_eq!(payload["schema_version"],1); assert_eq!(payload["source_kind"],"CONFIRMED_TRADE"); assert_eq!(payload["demo_status"],"REAL_ONLY"); assert_eq!(payload["scope"],"DELIVERY_POINT");
            assert!(payload.get("_public_market_invalidation").is_none()); assert!(payload.get("commission_amount_usd").is_none());
            let timestamp=payload["observed_at"].as_str().unwrap().parse::<chrono::DateTime<chrono::Utc>>().unwrap();
            if kind=="trade_delivered" {assert_eq!(timestamp,delivered["delivered_at"].as_str().unwrap().parse().unwrap()); assert_eq!(amount(&payload["final_total"]),"300".parse().unwrap()); assert!(payload.get("quantity").is_none());}
            else {assert_eq!(kind,"trade_paid");assert_eq!(timestamp,paid["paid_at"].as_str().unwrap().parse().unwrap());assert_eq!(amount(&payload["quantity"]),"3".parse().unwrap());assert!(payload.get("final_total").is_none());}
        }
        let notices:Vec<(Uuid,String,String,Value)>=sqlx::query_as("SELECT recipient_id,title,message,data FROM notifications ORDER BY recipient_id,title").fetch_all(&mut f.admin).await.unwrap();
        let delivery_org=if actor==BUYER {id(0x2000)}else{id(0x1000)};
        let delivery_recipients:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id").bind(delivery_org).fetch_all(&mut f.admin).await.unwrap();
        let actual:Vec<_>=notices.iter().filter(|n|n.1=="Trade Delivered").map(|n|n.0).collect();assert_eq!(actual,delivery_recipients);
        for (_,title,message,data) in &notices {assert_eq!(*data,json!({"trade_id":f.trade}));assert_eq!(message,if title=="Trade Delivered" {"Trade for 3 MT has been marked as delivered."}else{"Payment for trade of 3 MT has been confirmed."});}
        let audits:Vec<(String,Value)>=sqlx::query_as("SELECT action,changes FROM audit_logs ORDER BY timestamp,id").fetch_all(&mut f.admin).await.unwrap();
        assert_eq!(audits.len(),2);assert_eq!(audits[0].0,"trade.delivered");assert_eq!(audits[0].1,json!({"status":"DELIVERED","final_quantity_mt":"3","final_price_per_mt":"100","final_total_usd":"300"}));assert_eq!(audits[1],("trade.paid".into(),json!({"status":"PAID"})));
        if actor==BUYER {assert!(delivered["commission_amount_usd"].is_null());} else {assert!(!delivered["commission_amount_usd"].is_null());}
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn no_current_recipients_produces_no_notifications() {
    let mut f=Fixture::new().await; f.sql("UPDATE users SET organization_id=NULL WHERE organization_id='00000000-0000-0000-0000-000000001000'").await;
    ok(f.deliver(SELLER,None).await,"DELIVERED").await;ok(f.pay(SELLER,None).await,"PAID").await;
    assert_eq!(f.snapshot().await["notifications"],0);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn failed_effect_rolls_back_final_fields_and_receipt() {
    let mut f=Fixture::new().await;let before=f.snapshot().await;
    f.sql("CREATE TRIGGER settlement_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION settlement_fixture_reject_audit()").await;
    let result=f.deliver(SELLER,Some("rollback")).await;
    f.sql("DROP TRIGGER settlement_audit ON audit_logs").await;
    assert!(!result.0.is_success(),"{}",result.1);assert_eq!(f.snapshot().await,before);
}

async fn wait_for_lock(conn:&mut PgConnection) {
    tokio::time::timeout(std::time::Duration::from_secs(20),async {
        loop {let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid())").fetch_one(&mut *conn).await.unwrap();if waiting {break;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
    }).await.expect("observed settlement lock wait");
}
#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn deliver_and_pay_serialize_in_both_orders() {
    for first in ["deliver","pay"] {
        let mut f=Fixture::new().await;
        let mut holder=PgConnection::connect(&std::env::var("RUST_SETTLEMENT_ADMIN_DATABASE_URL").unwrap()).await.unwrap();
        sqlx::query("BEGIN").execute(&mut holder).await.unwrap();sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE").bind(f.trade).execute(&mut holder).await.unwrap();
        let router=f.router.clone();let trade=f.trade;
        let first_task=tokio::spawn(async move{send(&router,trade,first,Some(SELLER),&[],delivery().to_string(),0).await});
        wait_for_lock(&mut f.admin).await;
        let router=f.router.clone();let second=if first=="deliver"{"pay"}else{"deliver"};
        let second_task=tokio::spawn(async move{send(&router,trade,second,Some(SELLER),&[],delivery().to_string(),0).await});
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
        let a=first_task.await.unwrap();let b=second_task.await.unwrap();
        if first=="deliver" {ok(a,"DELIVERED").await;ok(b,"PAID").await;assert_eq!(f.snapshot().await["audit"],2);}
        else {assert_eq!(a.0,StatusCode::BAD_REQUEST,"{}",a.1);assert_eq!(a.1["detail"]["code"],"TRADE_NOT_DELIVERED");ok(b,"DELIVERED").await;let after=f.snapshot().await;assert_eq!(after["audit"],1);assert!(after["trades"][0]["paid_at"].is_null());}
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn concurrent_identical_requests_commit_one_receipt_and_effect_set() {
    let mut f=Fixture::new().await;
    let (a,b)=tokio::join!(f.deliver(SELLER,Some("one")),f.deliver(SELLER,Some("one")));
    assert_eq!(ok(a,"DELIVERED").await,ok(b,"DELIVERED").await);
    let (a,b)=tokio::join!(f.pay(SELLER,Some("one")),f.pay(SELLER,Some("one")));
    assert_eq!(ok(a,"PAID").await,ok(b,"PAID").await);
    let after=f.snapshot().await;assert_eq!(after["receipts"],2);assert_eq!(after["audit"],2);assert_eq!(after["outbox"],2);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn retryable_abort_rebuilds_once_without_duplicate_effects() {
    let mut f=Fixture::new().await;f.sql("SELECT confirm_fixture_abort_install()").await;
    let result=f.deliver(SELLER,Some("retry")).await;f.sql("SELECT confirm_fixture_abort_remove()").await;
    ok(result,"DELIVERED").await;
    let attempts:i64=sqlx::query_scalar("SELECT last_value FROM confirm_fixture_attempts").fetch_one(&mut f.admin).await.unwrap();assert_eq!(attempts,2);
    let after=f.snapshot().await;assert_eq!(after["receipts"],1);assert_eq!(after["audit"],1);assert_eq!(after["outbox"],1);
}

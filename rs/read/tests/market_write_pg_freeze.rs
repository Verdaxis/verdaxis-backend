#![cfg(feature = "contract-tests")]

use axum::{Router, body::Body, extract::ConnectInfo, http::{Request, StatusCode}};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::freeze::contract_router;

const KEY: &str = "synthetic-freeze-signing-key-at-least-32";
const ADMIN: u128 = 0x4001;
const OTHER_ADMIN: u128 = 0x4002;
const BUYER: u128 = 0x1001;
fn id(n: u128) -> Uuid { Uuid::from_u128(n) }

struct Fixture { admin: PgConnection, pool: PgPool, router: Router, trade: Uuid, admin_url: String }
impl Fixture {
    async fn new(state: &str) -> Self {
        let a = std::env::var("RUST_FREEZE_ADMIN_DATABASE_URL").expect("disposable admin URL");
        let r = std::env::var("RUST_FREEZE_DATABASE_URL").expect("disposable runtime URL");
        let au = url::Url::parse(&a).unwrap();
        let ru = url::Url::parse(&r).unwrap();
        assert_eq!(au.host_str(), Some("127.0.0.1"));
        assert!(au.port().is_some());
        assert!(au.path().ends_with("_test"));
        assert_eq!((au.host_str(),au.port(),au.path()),(ru.host_str(),ru.port(),ru.path()));
        assert_ne!(au.username(),ru.username());
        let mut admin = PgConnection::connect(&a).await.unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version").fetch_one(&mut admin).await.unwrap();
        assert_eq!(revision,"p89_20261022_trade_freeze");
        sqlx::raw_sql(include_str!("fixtures/phase9_trades.sql")).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase9_confirm.sql")).execute(&mut admin).await.unwrap();
        let policy: Value = serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let p=policy["products"].as_array().unwrap().iter().find(|p| p["qualification_profile"]=="scalar" && p["execution_mode"]=="ORDERBOOK" && p.get("specification_standard").is_none()).unwrap();
        sqlx::query("SELECT trades_fixture_seed($1,$2)").bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap()).bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap()).execute(&mut admin).await.unwrap();
        sqlx::query("SELECT set_config('confirm.policy',$1,false)").bind(policy["products"].to_string()).execute(&mut admin).await.unwrap();
        let trade=sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)").fetch_one(&mut admin).await.unwrap();
        sqlx::query("INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password) VALUES($1,'other-freeze-admin@example.test','unused','ADMIN','APPROVED',true,false)").bind(id(OTHER_ADMIN)).execute(&mut admin).await.unwrap();
        let pool=PgPoolOptions::new().max_connections(8).connect(&r).await.unwrap();
        let router=contract_router(pool.clone(),KEY);
        let mut f=Self {admin,pool,router,trade,admin_url:a};
        f.state(state).await;
        f
    }
    async fn state(&mut self,state:&str) {
        sqlx::query("UPDATE trades SET status=$2,confirmed_at=CASE WHEN $2 IN ('CONFIRMED','DELIVERED','PAID') THEN now() END,delivered_at=CASE WHEN $2 IN ('DELIVERED','PAID') THEN now() END,paid_at=CASE WHEN $2='PAID' THEN now() END,final_quantity_mt=CASE WHEN $2 IN ('DELIVERED','PAID') THEN quantity_mt END,final_price_per_mt=CASE WHEN $2 IN ('DELIVERED','PAID') THEN price_per_mt_usd END,final_total_usd=CASE WHEN $2 IN ('DELIVERED','PAID') THEN quantity_mt*price_per_mt_usd END WHERE id=$1").bind(self.trade).bind(state).execute(&mut self.admin).await.unwrap();
    }
    async fn call(&self, action:&str,actor:u128,body:Value)->(StatusCode,Value) {
        send(&self.router,"POST",&format!("/api/admin/trades/{}/{action}",self.trade),Some(token(actor,0,600,"access")),body,&[],None).await
    }
    async fn freeze(&self,actor:u128,reason:&str)->(StatusCode,Value) {
        self.call("freeze",actor,json!({"reason_code":reason,"note":"  review requested  "})).await
    }
    async fn snapshot(&mut self)->Value {
        sqlx::query_scalar("SELECT jsonb_build_object('base',confirm_fixture_snapshot(),'freezes',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM trade_freezes f),'audit',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_logs a))").fetch_one(&mut self.admin).await.unwrap()
    }
    async fn sql(&mut self,sql:&str) { sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned())).execute(&mut self.admin).await.unwrap(); }
    async fn counts(&mut self)->(i64,i64,i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM trade_freezes WHERE status='ACTIVE'),(SELECT count(*) FROM trade_freezes),(SELECT count(*) FROM audit_logs)").fetch_one(&mut self.admin).await.unwrap()
    }
    async fn audit(&mut self,action:&str)->Value {
        sqlx::query_scalar("SELECT jsonb_build_object('actor',user_id,'resource_type',resource_type,'resource_id',resource_id,'changes',changes,'ip',ip_address,'request_id',request_id) FROM audit_logs WHERE action=$1 ORDER BY timestamp DESC LIMIT 1").bind(action).fetch_one(&mut self.admin).await.unwrap()
    }
    async fn page(&self,query:&str)->(StatusCode,Value) {
        send(&self.router,"GET",&format!("/api/admin/trades/frozen{query}"),Some(token(ADMIN,0,600,"access")),Value::Null,&[],None).await
    }
}
fn token(actor:u128,iat_offset:i64,exp_offset:i64,kind:&str)->String {
    let now=chrono::Utc::now().timestamp();
    encode(&Header::default(),&json!({"sub":id(actor),"type":kind,"iat":now+iat_offset,"exp":now+exp_offset,"iss":"freeze-tests","aud":"freeze-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap()
}
async fn send(router:&Router,method:&str,path:&str,token:Option<String>,body:Value,headers:&[(&str,&str)],peer:Option<&str>)->(StatusCode,Value) {
    let mut request=Request::builder().method(method).uri(path).header("content-type","application/json");
    if let Some(token)=token { request=request.header("authorization",format!("Bearer {token}")); }
    for (key,value) in headers { request=request.header(*key,*value); }
    let mut request=request.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    if let Some(peer)=peer { request.extensions_mut().insert(ConnectInfo(peer.parse::<std::net::SocketAddr>().unwrap())); }
    let response=router.clone().oneshot(request).await.unwrap();
    let status=response.status();
    let bytes=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
    (status,serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
fn code(body:&Value)->&str { body["detail"]["code"].as_str().unwrap() }

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_accepts_a_delivered_unpaid_trade() {
    let mut f=Fixture::new("DELIVERED").await;
    let before=f.snapshot().await;
    let (s,b)=f.freeze(ADMIN,"COMPLIANCE_REVIEW").await;
    assert_eq!(s,StatusCode::CREATED,"{b}");
    assert_eq!(b["status"],"ACTIVE");
    assert_eq!(b["trade_id"],f.trade.to_string());
    assert_eq!(b["note"],"review requested");
    assert_eq!(f.counts().await,(1,1,1));
    let after=f.snapshot().await;
    for key in ["trades","orders","inventory","referrals","notifications","outbox","receipts","benchmarks","targets"] { assert_eq!(before["base"][key],after["base"][key],"{key}"); }
    let audit=f.audit("TRADE_FROZEN").await;
    assert_eq!(audit["actor"],id(ADMIN).to_string());
    assert_eq!(audit["resource_type"],"trade");
    assert_eq!(audit["resource_id"],f.trade.to_string());
    assert_eq!(audit["changes"],json!({"status":{"before":null,"after":"ACTIVE"},"reason_code":"COMPLIANCE_REVIEW","note":"review requested","actor":id(ADMIN),"actor_is_party":false}));
    println!("AUDIT_READBACK={audit}");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_is_idempotent_and_clear_is_audited() {
    let mut f=Fixture::new("CONFIRMED").await;
    let (s,first)=f.freeze(ADMIN,"OTHER").await; assert_eq!(s,StatusCode::CREATED);
    let before=f.snapshot().await;
    let (s,replay)=f.call("freeze",ADMIN,json!({"reason_code":"OTHER","note":"changed note"})).await;
    assert_eq!(s,StatusCode::OK); assert_eq!(replay,first); assert_eq!(f.snapshot().await,before);
    for (actor,reason) in [(OTHER_ADMIN,"OTHER"),(ADMIN,"COURT_ORDER")] {
        let (s,b)=f.freeze(actor,reason).await; assert_eq!(s,StatusCode::CONFLICT);assert_eq!(code(&b),"TRADE_ALREADY_FROZEN");assert_eq!(f.snapshot().await,before);
    }
    let (s,clear)=f.call("unfreeze",OTHER_ADMIN,json!({"note":"  review complete  "})).await;
    assert_eq!(s,StatusCode::OK,"{clear}");assert_eq!(clear["status"],"CLEARED");assert_eq!(clear["version"],2);
    for key in ["id","trade_id","reason_code","note","frozen_at","frozen_by_user_id"] { assert_eq!(clear[key],first[key],"{key}"); }
    assert_eq!(clear["clear_note"],"review complete");assert_eq!(clear["cleared_by_user_id"],id(OTHER_ADMIN).to_string());assert!(!clear["cleared_at"].is_null());
    let audit=f.audit("TRADE_UNFROZEN").await;
    assert_eq!(audit["changes"],json!({"status":{"before":"ACTIVE","after":"CLEARED"},"reason_code":"OTHER","note":"review complete","actor":id(OTHER_ADMIN),"actor_is_party":false}));
    let before=f.snapshot().await;let(s,b)=f.call("unfreeze",ADMIN,json!({"note":"again"})).await;assert_eq!(s,StatusCode::CONFLICT);assert_eq!(code(&b),"TRADE_NOT_FROZEN");assert_eq!(f.snapshot().await,before);
    let(s,next)=f.freeze(ADMIN,"COURT_ORDER").await;assert_eq!(s,StatusCode::CREATED);assert_ne!(next["id"],first["id"]);assert_eq!(f.counts().await,(1,2,3));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_refuses_a_terminal_trade() {
    for state in ["PAID","DECLINED","CANCELLED"] {
        let mut f=Fixture::new(state).await;let before=f.snapshot().await;
        let(s,b)=f.freeze(ADMIN,"OTHER").await;assert_eq!(s,StatusCode::CONFLICT,"{state}: {b}");assert_eq!(code(&b),"TRADE_NOT_SETTLEABLE");assert_eq!(f.snapshot().await,before);
        // Historical ACTIVE rows may coexist with a terminal trade; matching replay wins.
        sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id) VALUES($1,$2,'ACTIVE','OTHER','historical review',$3)").bind(Uuid::new_v4()).bind(f.trade).bind(id(ADMIN)).execute(&mut f.admin).await.unwrap();
        let(s,b)=f.freeze(ADMIN,"OTHER").await;assert_eq!(s,StatusCode::OK,"{b}");
        let(s,b)=f.call("unfreeze",ADMIN,json!({"note":"clear history"})).await;assert_eq!(s,StatusCode::OK,"{b}");assert_eq!(b["status"],"CLEARED");
        let stored:String=sqlx::query_scalar("SELECT status::text FROM trades WHERE id=$1").bind(f.trade).fetch_one(&mut f.admin).await.unwrap();assert_eq!(stored,state);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_does_not_change_stock_status_or_remainder() {
    for state in ["PENDING_CONFIRMATION","CONFIRMED","DELIVERED"] {
        let mut f=Fixture::new(state).await;let before=f.snapshot().await;
        let(s,b)=f.freeze(ADMIN,"PAYMENT_DISPUTE").await;assert_eq!(s,StatusCode::CREATED,"{b}");
        let(s,b)=f.call("unfreeze",ADMIN,json!({"note":"complete"})).await;assert_eq!(s,StatusCode::OK,"{b}");
        let after=f.snapshot().await;for key in ["trades","orders","inventory"] {assert_eq!(before["base"][key],after["base"][key]);}
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn non_admin_is_refused() {
    let mut f=Fixture::new("DELIVERED").await;let before=f.snapshot().await;
    for (method,path,body) in [("POST",format!("/api/admin/trades/{}/freeze",f.trade),json!({"reason_code":"OTHER","note":"review"})),("POST",format!("/api/admin/trades/{}/unfreeze",f.trade),json!({"note":"review"})),("GET","/api/admin/trades/frozen".to_owned(),Value::Null)] {
        let(s,b)=send(&f.router,method,&path,Some(token(BUYER,0,600,"access")),body,&[],None).await;assert_eq!(s,StatusCode::FORBIDDEN,"{b}");assert_eq!(f.snapshot().await,before);
    }
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn authentication_and_validation_precede_resource_checks() {
    let mut f=Fixture::new("DELIVERED").await;
    for (token,expected) in [(None,401),(Some("invalid".into()),401),(Some(token(ADMIN,-600,-300,"access")),401),(Some(token(ADMIN,0,600,"refresh")),401),(Some(token(BUYER,0,600,"access")),403)] {
        let before=f.snapshot().await;
        let(s,b)=send(&f.router,"POST","/api/admin/trades/not-a-uuid/freeze",token,json!({}),&[],None).await;assert_eq!(s.as_u16(),expected,"{b}");assert_eq!(f.snapshot().await,before);
    }
    for change in ["pending","rejected","password"] {
        let mut f=Fixture::new("DELIVERED").await;
        sqlx::query("SELECT trades_fixture_change($1,$2)").bind(id(ADMIN)).bind(change).execute(&mut f.admin).await.unwrap();
        let before=f.snapshot().await;let(s,b)=send(&f.router,"POST","/api/admin/trades/not-a-uuid/freeze",Some(token(ADMIN,0,600,"access")),json!({}),&[],None).await;assert_eq!(s,StatusCode::FORBIDDEN,"{change}: {b}");assert_eq!(f.snapshot().await,before);
    }
    for field in ["password_changed_at","authentication_revoked_at"] {
        let mut f=Fixture::new("DELIVERED").await;
        f.sql(&format!("UPDATE users SET {field}=now()+interval '1 minute' WHERE id='{}'",id(ADMIN))).await;
        let before=f.snapshot().await;let(s,b)=f.freeze(ADMIN,"OTHER").await;assert_eq!(s,StatusCode::UNAUTHORIZED,"{field}: {b}");assert_eq!(f.snapshot().await,before);
    }
    let mut f=Fixture::new("DELIVERED").await;
    for action in ["freeze","unfreeze"] {
        let path=format!("/api/admin/trades/{}/{action}",Uuid::new_v4());
        let(s,b)=send(&f.router,"POST",&path,Some(token(ADMIN,0,600,"access")),json!({}),&[],None).await;assert_eq!(s,StatusCode::UNPROCESSABLE_ENTITY,"{b}");
        let body=if action=="freeze" {json!({"reason_code":"OTHER","note":"valid"})} else {json!({"note":"valid"})};
        let(s,b)=send(&f.router,"POST",&path,Some(token(ADMIN,0,600,"access")),body,&[],None).await;assert_eq!(s,StatusCode::NOT_FOUND,"{b}");
    }
    let(s,b)=f.call("unfreeze",ADMIN,json!({"note":"valid"})).await;assert_eq!(s,StatusCode::CONFLICT);assert_eq!(code(&b),"TRADE_NOT_FROZEN");assert_eq!(f.counts().await,(0,0,0));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn support_context_and_preauth_are_outer_boundaries() {
    let mut f=Fixture::new("DELIVERED").await;let before=f.snapshot().await;
    for action in ["freeze","unfreeze"] {
        for header in ["malformed"," ","00000000-0000-0000-0000-000000006000"] {
            let(s,b)=send(&f.router,"POST",&format!("/api/admin/trades/{}/{action}",f.trade),None,json!({}),&[("x-verdaxis-market-support-context",header)],None).await;assert_eq!(s,StatusCode::FORBIDDEN);assert_eq!(code(&b),"MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
        }
    }
    let(s,b)=send(&f.router,"GET","/api/admin/trades/frozen",Some(token(ADMIN,0,600,"access")),Value::Null,&[("x-verdaxis-market-support-context","malformed")],None).await;assert_eq!(s,StatusCode::OK,"{b}");assert_eq!(f.snapshot().await,before);
    let router=contract_router(f.pool.clone(),KEY);
    for _ in 0..300 {let(s,_)=send(&router,"GET","/api/admin/trades/frozen",None,Value::Null,&[],Some("198.51.100.4:1234")).await;assert_eq!(s,StatusCode::UNAUTHORIZED);}
    let request=Request::builder().uri("/api/admin/trades/frozen").body(Body::empty()).unwrap();
    let mut request=request;request.extensions_mut().insert(ConnectInfo("198.51.100.4:1234".parse::<std::net::SocketAddr>().unwrap()));
    let response=router.oneshot(request).await.unwrap();assert_eq!(response.status(),StatusCode::TOO_MANY_REQUESTS);assert_eq!(response.headers()["retry-after"],"60");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn party_conflict_and_bounded_ingress_identity_are_audited() {
    for (membership,peer,expected_ip) in [(false,"198.51.100.7:1234","198.51.100.7"),(true,"127.0.0.1:1234","203.0.113.9")] {
        let mut f=Fixture::new("DELIVERED").await;
        if membership { f.sql(&format!("UPDATE users SET organization_id='{}' WHERE id='{}'",id(0x1000),id(ADMIN))).await; }
        let long="x".repeat(200);
        let(s,b)=send(&f.router,"POST",&format!("/api/admin/trades/{}/freeze",f.trade),Some(token(ADMIN,0,600,"access")),json!({"reason_code":"OTHER","note":"review"}),&[("x-forwarded-for","203.0.113.9"),("x-request-id",&long)],Some(peer)).await;assert_eq!(s,StatusCode::CREATED,"{b}");
        let audit=f.audit("TRADE_FROZEN").await;assert_eq!(audit["changes"]["actor_is_party"],membership);assert_eq!(audit["ip"],expected_ip);assert!(audit["request_id"].as_str().unwrap().len()<=36);assert_ne!(audit["request_id"],long);
    }
    let mut f=Fixture::new("DELIVERED").await;
    // The recorded participant remains a party even after current membership is removed.
    f.sql(&format!("UPDATE users SET role='ADMIN',organization_id=NULL WHERE id='{}'",id(BUYER))).await;
    let(s,b)=f.freeze(BUYER,"OTHER").await;assert_eq!(s,StatusCode::CREATED,"{b}");assert_eq!(f.audit("TRADE_FROZEN").await["changes"]["actor_is_party"],true);
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn failed_audit_rolls_back_each_transition() {
    let mut f=Fixture::new("DELIVERED").await;
    f.sql("SELECT trades_fixture_reject_audit()").await;let before=f.snapshot().await;
    let(s,_)=f.freeze(ADMIN,"OTHER").await;assert!(!s.is_success());assert_eq!(f.snapshot().await,before);
    f.sql("SELECT trades_fixture_allow_audit()").await;assert_eq!(f.freeze(ADMIN,"OTHER").await.0,StatusCode::CREATED);
    f.sql("SELECT trades_fixture_reject_audit()").await;let before=f.snapshot().await;
    let(s,_)=f.call("unfreeze",ADMIN,json!({"note":"clear review"})).await;assert!(!s.is_success());assert_eq!(f.snapshot().await,before);
    f.sql("SELECT trades_fixture_allow_audit()").await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn frozen_list_is_bounded_and_newest_first() {
    let mut f=Fixture::new("DELIVERED").await;
    let(s,b)=f.page("").await;assert_eq!(s,StatusCode::OK);assert_eq!(b,json!({"items":[],"total":0,"skip":0,"limit":25}));
    let mut ids=Vec::new();
    for n in 1..=4 {
        let trade=Uuid::new_v4();
        sqlx::query("INSERT INTO trades SELECT (jsonb_populate_record(NULL::trades,to_jsonb(t)||jsonb_build_object('id',$1::uuid))).* FROM trades t WHERE id=$2").bind(trade).bind(f.trade).execute(&mut f.admin).await.unwrap();
        let freeze=id(0x8000+n);ids.push(freeze);
        sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id,frozen_at) VALUES($1,$2,'ACTIVE','OTHER','private review',$3,'2026-10-01T00:00:00Z')").bind(freeze).bind(trade).bind(id(ADMIN)).execute(&mut f.admin).await.unwrap();
    }
    sqlx::query("UPDATE trade_freezes SET status='CLEARED',cleared_by_user_id=$1,cleared_at=now(),clear_note='complete',version=2 WHERE id=$2").bind(id(ADMIN)).bind(ids[0]).execute(&mut f.admin).await.unwrap();
    let before=f.snapshot().await;let(s,b)=f.page("?skip=1&limit=2").await;assert_eq!(s,StatusCode::OK,"{b}");assert_eq!(b["total"],3);assert_eq!(b["skip"],1);assert_eq!(b["limit"],2);assert_eq!(b["items"][0]["id"],ids[2].to_string());assert_eq!(b["items"][1]["id"],ids[1].to_string());
    for item in b["items"].as_array().unwrap() {for field in ["trade_id","trade_status","reason_code","note","frozen_at","frozen_by_user_id"] {assert!(!item[field].is_null(),"{field}");}}
    assert_eq!(f.page("?skip=3").await.1["items"],json!([]));
    for query in ["?skip=-1","?limit=0","?limit=101"] {assert_eq!(f.page(query).await.0,StatusCode::UNPROCESSABLE_ENTITY);}
    assert_eq!(f.snapshot().await,before);
}

async fn wait_for_lock(admin:&mut PgConnection) {
    for _ in 0..200 {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid())").fetch_one(&mut *admin).await.unwrap();
        if waiting {println!("OBSERVED_TRADE_LOCK_WAIT=true");return;}
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("trade lock wait was not observed");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_race_returns_409_not_500() {
    for (actor,reason,expected) in [(ADMIN,"OTHER",StatusCode::OK),(OTHER_ADMIN,"COURT_ORDER",StatusCode::CONFLICT)] {
        let mut f=Fixture::new("DELIVERED").await;
        let mut blocker=PgConnection::connect(&f.admin_url).await.unwrap();let mut held=blocker.begin().await.unwrap();
        sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE").bind(f.trade).fetch_one(&mut *held).await.unwrap();
        let router=f.router.clone();let path=format!("/api/admin/trades/{}/freeze",f.trade);
        let request=tokio::spawn(async move {send(&router,"POST",&path,Some(token(actor,0,600,"access")),json!({"reason_code":reason,"note":"racing review"}),&[],None).await});
        wait_for_lock(&mut f.admin).await;
        sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id) VALUES($1,$2,'ACTIVE','OTHER','first review',$3)").bind(Uuid::new_v4()).bind(f.trade).bind(id(ADMIN)).execute(&mut *held).await.unwrap();
        held.commit().await.unwrap();let(s,b)=request.await.unwrap();assert_eq!(s,expected,"{b}");assert_eq!(f.counts().await,(1,1,0));
    }
    // Real component writers also serialize on the same trade, with independent actors.
    let mut f=Fixture::new("DELIVERED").await;
    let r=f.router.clone();let path=format!("/api/admin/trades/{}/freeze",f.trade);
    let a=send(&r,"POST",&path,Some(token(ADMIN,0,600,"access")),json!({"reason_code":"OTHER","note":"first review"}),&[],None);
    let b=send(&r,"POST",&path,Some(token(OTHER_ADMIN,0,600,"access")),json!({"reason_code":"COURT_ORDER","note":"second review"}),&[],None);
    let(a,b)=tokio::join!(a,b);let mut statuses=[a.0.as_u16(),b.0.as_u16()];statuses.sort();assert_eq!(statuses,[201,409],"{a:?} {b:?}");assert_eq!(f.counts().await,(1,1,1));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn freeze_locks_trade_before_insert_against_concurrent_pay() {
    // Controlled SQL status writer, not the payment component. The two-route race is separate.
    let mut f=Fixture::new("DELIVERED").await;
    let mut blocker=PgConnection::connect(&f.admin_url).await.unwrap();let mut held=blocker.begin().await.unwrap();
    sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE").bind(f.trade).fetch_one(&mut *held).await.unwrap();
    let router=f.router.clone();let path=format!("/api/admin/trades/{}/freeze",f.trade);
    let request=tokio::spawn(async move {send(&router,"POST",&path,Some(token(ADMIN,0,600,"access")),json!({"reason_code":"OTHER","note":"review"}),&[],None).await});
    wait_for_lock(&mut f.admin).await;
    sqlx::query("UPDATE trades SET status='PAID',paid_at=now() WHERE id=$1").bind(f.trade).execute(&mut *held).await.unwrap();held.commit().await.unwrap();
    let(s,b)=request.await.unwrap();assert_eq!(s,StatusCode::CONFLICT,"{b}");assert_eq!(code(&b),"TRADE_NOT_SETTLEABLE");assert_eq!(f.counts().await,(0,0,0));
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn clear_and_refreeze_serialize_and_bounded_waits_leave_no_effects() {
    let mut f=Fixture::new("DELIVERED").await;
    assert_eq!(f.freeze(ADMIN,"OTHER").await.0,StatusCode::CREATED);
    let mut blocker=PgConnection::connect(&f.admin_url).await.unwrap();let mut held=blocker.begin().await.unwrap();
    sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE").bind(f.trade).fetch_one(&mut *held).await.unwrap();
    let router=f.router.clone();let path=format!("/api/admin/trades/{}/unfreeze",f.trade);
    let request=tokio::spawn(async move {send(&router,"POST",&path,Some(token(OTHER_ADMIN,0,600,"access")),json!({"note":"complete"}),&[],None).await});
    wait_for_lock(&mut f.admin).await;held.commit().await.unwrap();let(s,b)=request.await.unwrap();assert_eq!(s,StatusCode::OK,"{b}");
    assert_eq!(f.freeze(ADMIN,"COURT_ORDER").await.0,StatusCode::CREATED);assert_eq!(f.counts().await,(1,2,3));
    let mut held=blocker.begin().await.unwrap();sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE").bind(f.trade).fetch_one(&mut *held).await.unwrap();let before=f.snapshot().await;
    let(s,b)=f.call("unfreeze",OTHER_ADMIN,json!({"note":"complete"})).await;assert_eq!(s,StatusCode::CONFLICT,"{b}");assert_eq!(code(&b),"MARKET_BUSY");assert_eq!(f.snapshot().await,before);held.rollback().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn named_unique_mapping_and_database_guard_remain_exact() {
    let mut f=Fixture::new("DELIVERED").await;
    assert_eq!(f.freeze(ADMIN,"OTHER").await.0,StatusCode::CREATED);
    let error=sqlx::query("INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id) VALUES($1,$2,'ACTIVE','OTHER','review',$3)").bind(Uuid::new_v4()).bind(f.trade).bind(id(ADMIN)).execute(&f.pool).await.unwrap_err();
    assert_eq!(error.as_database_error().unwrap().constraint(),Some("uq_trade_freezes_one_active"));
    let mapped=verdaxis_read::order_write::MarketWriteError::from(error);assert_eq!(mapped.status,StatusCode::CONFLICT);assert_eq!(mapped.code.as_str(),"TRADE_ALREADY_FROZEN");
    sqlx::query("UPDATE trades SET status=status WHERE id=$1").bind(f.trade).execute(&f.pool).await.unwrap();
    let before=f.snapshot().await;
    let error=sqlx::query("UPDATE trades SET status='PAID',paid_at=now() WHERE id=$1").bind(f.trade).execute(&f.pool).await.unwrap_err();assert_eq!(error.as_database_error().unwrap().code().as_deref(),Some("ZF001"));assert_eq!(f.snapshot().await,before);
}

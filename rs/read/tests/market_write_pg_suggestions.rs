#![cfg(feature = "contract-tests")]

use axum::{Router, body::{Body, to_bytes}, http::Request};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{AppState, Config, MarketPolicy, auth::{VerificationSettings, limits::SharedLimits}, routes::market_write::matchmaking::{MatchmakingAuthority, routes}};

const SECRET: &str = "disposable-suggestions-signing-key-minimum-32";
const FIXTURE: &str = include_str!("fixtures/phase9_suggestions.sql");
fn section(name: &str) -> &'static str {
    FIXTURE.split(&format!("-- section: {name}\n")).nth(1).unwrap().split("-- section:").next().unwrap()
}
fn id(n: u128) -> Uuid { Uuid::from_u128(n) }
fn token(actor: Uuid) -> String {
    let now = Utc::now();
    jsonwebtoken::encode(&jsonwebtoken::Header::default(), &json!({"sub":actor,"type":"access","iss":"suggestions-test","aud":"suggestions-test","jti":Uuid::new_v4(),"iat":now.timestamp(),"exp":(now+Duration::hours(1)).timestamp()}), &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes())).unwrap()
}
struct Fixture { admin: PgConnection, schema: String, app: Router, pool: sqlx::PgPool }
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").expect("explicit disposable migrator required");
        let runtime_url = std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime required");
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert_eq!(r.host_str(), a.host_str()); assert_eq!(r.port(), a.port()); assert_eq!(r.path(), a.path()); assert_eq!(r.username(), "rust_app");
        let schema = format!("suggestions_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}; SET search_path TO {schema}"))).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(section("schema")).execute(&mut admin).await.unwrap();
        sqlx::query(section("seed")).bind(json!({"organizations":[id(10),id(20),id(30),id(40)]})).execute(&mut admin).await.unwrap();
        sqlx::query(section("users")).bind(json!([
            {"id":id(1),"organization_id":id(10),"role":"BUYER"},
            {"id":id(2),"organization_id":id(20),"role":"SUPPLIER"},
            {"id":id(3),"organization_id":id(30),"role":"SUPPLIER"},
            {"id":id(4),"organization_id":id(10),"role":"BUYER"},
            {"id":id(5),"organization_id":id(40),"role":"BUYER"}
        ])).execute(&mut admin).await.unwrap();
        let policy: MarketPolicy = serde_json::from_value(json!({"source_sha":verdaxis_read::compiled_sha(),"products":[{"id":id(100),"code":"SYNTHETIC_SCALAR","name":"Synthetic scalar","fuel_type":"Methanol","fuel_grade":"Synthetic","execution_mode":"ORDERBOOK","lanes":[id(101)],"qualification_profile":"scalar","display_order":1}],"points":[{"id":id(101),"name":"Synthetic point","region":"Synthetic region","timezone":"UTC","display_order":1}]})).unwrap();
        sqlx::query(section("product")).bind(id(100)).bind("Synthetic scalar").bind("Methanol").bind("Synthetic").execute(&mut admin).await.unwrap();
        sqlx::query(section("point")).bind(id(101)).bind("Synthetic point").bind("Synthetic region").execute(&mut admin).await.unwrap();
        let orders = [(200,10,1,"BID","REAL",550),(201,20,2,"ASK","REAL",500),(202,20,2,"ASK","UNKNOWN",491),(203,30,3,"ASK","REAL",492),(204,20,2,"ASK","REAL",493),(205,20,2,"ASK","REAL",494)];
        let rows: Vec<Value> = orders.into_iter().map(|(n,org,owner,side,provenance,price)| json!({"id":id(n),"organization_id":id(org),"owner_user_id":id(owner),"side":side,"provenance":provenance,"price":price,"expires_at":if n==205 {Some(Utc::now()-Duration::hours(1))} else {None}})).collect();
        sqlx::query(section("orders")).bind(json!(rows)).bind(id(100)).bind(id(101)).bind("declared standard").execute(&mut admin).await.unwrap();
        sqlx::query(section("reject")).bind(id(3)).execute(&mut admin).await.unwrap();
        sqlx::query(section("incomplete")).bind(id(204)).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app; GRANT UPDATE ON organizations,users TO rust_app; GRANT INSERT ON match_suggestions TO rust_app"))).execute(&mut admin).await.unwrap();
        let search = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new().max_connections(4).after_connect(move |conn,_| {let sql=search.clone(); Box::pin(async move {sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(conn).await?; Ok(())})}).connect(&runtime_url).await.unwrap();
        let health = PgPoolOptions::new().max_connections(1).connect_lazy(&runtime_url).unwrap();
        let config = Config {kyc_execution_gate_enabled:true,environment:"test".into(),release_sha:verdaxis_read::compiled_sha().into(),migration_revision:"kycg_20261014_staging".into(),database_url:runtime_url,db_session_time_zone:"UTC".into(),prepared_budget:None,bind:"127.0.0.1:0".parse().unwrap(),read_admission_slots:4,pool_max:5,python_budget:20,configured_max_connections:100};
        let state = AppState::new(config,verdaxis_read::runtime::db::Pools {read:pool.clone(),health},policy).unwrap();
        let authority = MatchmakingAuthority::new(VerificationSettings::new(SECRET.into(),None,"suggestions-test".into(),"suggestions-test".into()),SharedLimits::default());
        let app = routes(Arc::new(authority)).with_state(state);
        Self {admin,schema,app,pool}
    }
    async fn request(&self, actor: Uuid, method: &str, path: &str) -> (u16,Value) { request(self.app.clone(),actor,method,path).await }
    async fn state(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('audit',(SELECT count(*) FROM audit_logs),'notifications',(SELECT count(*) FROM notifications),'outbox',(SELECT count(*) FROM market_event_outbox),'orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM orderbook_orders o),'trades',(SELECT count(*) FROM trades))").fetch_one(&mut self.admin).await.unwrap()
    }
    async fn finish(mut self) { drop(self.app); self.pool.close().await; sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {} CASCADE",self.schema))).execute(&mut self.admin).await.unwrap(); }
}
async fn request(app: Router, actor: Uuid, method: &str, path: &str) -> (u16,Value) {
    let response = app.oneshot(Request::builder().method(method).uri(path).header("authorization",format!("Bearer {}",token(actor))).body(Body::empty()).unwrap()).await.unwrap();
    let status=response.status().as_u16(); let bytes=to_bytes(response.into_body(),1024*1024).await.unwrap();
    (status,serde_json::from_slice(&bytes).unwrap())
}
const LIST: &str = "/api/matchmaking/suggestions";
fn dismiss(order: u128) -> String {format!("{LIST}/{}/dismiss",id(order))}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn suggestions_never_project_a_nonpublic_row() {
    let f=Fixture::new().await;
    let (status,body)=f.request(id(1),"GET",LIST).await;
    assert_eq!(status,200); let rows=body.as_array().unwrap();
    assert_eq!(rows.len(),1,"only the public positive control is suggested: {body}");
    assert_eq!(rows[0]["id"],id(201).to_string());
    assert_eq!(rows[0]["ask_order"]["price_per_mt_usd"],"500");
    let text=body.to_string();
    for hidden in [202,203,204,205] {assert!(!text.contains(&id(hidden).to_string()),"nonpublic candidate {hidden}: {body}");}
    assert!(!text.contains("private-document-reference"));
    assert!(rows[0]["ask_order"].get("organization_id").is_none());
    assert!(rows[0]["ask_order"].get("owner_user_id").is_none());
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn suggestions_use_the_shared_public_relation() {
    let mut f=Fixture::new().await;
    assert_eq!(f.request(id(1),"GET",LIST).await.1[0]["id"],id(201).to_string());
    sqlx::query(section("kyc")).bind(id(2)).bind("REJECTED").bind(id(20)).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(id(1),"GET",LIST).await,(200,json!([])));
    sqlx::query(section("kyc")).bind(id(2)).bind("APPROVED").bind(id(20)).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(id(2),"GET",LIST).await.1[0]["id"],id(200).to_string());
    assert_eq!(f.request(id(1),"PATCH",&dismiss(201)).await,(200,json!({"status":"dismissed"})));
    assert_eq!(f.request(id(4),"GET",LIST).await,(200,json!([])));
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn role_without_market_access_gets_an_empty_list() {
    let mut f=Fixture::new().await;
    sqlx::query(section("role")).bind(id(1)).bind("ADMIN").execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(id(1),"GET",LIST).await,(200,json!([])));
    let first=f.request(id(1),"PATCH",&dismiss(201)).await;
    let missing=f.request(id(1),"PATCH",&dismiss(999)).await;
    assert_eq!(first,missing); assert_eq!(first.0,403);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM match_suggestions").fetch_one(&mut f.admin).await.unwrap(); assert_eq!(count,0);
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL"]
async fn dismiss_is_idempotent_and_writes_no_audit() {
    let mut f=Fixture::new().await; let before=f.state().await;
    let path=dismiss(201);
    let (left,right)=tokio::join!(request(f.app.clone(),id(1),"PATCH",&path),request(f.app.clone(),id(4),"PATCH",&path));
    assert_eq!(left.0,200); assert_eq!(right.0,200);
    let mut statuses=vec![left.1["status"].as_str().unwrap().to_owned(),right.1["status"].as_str().unwrap().to_owned()]; statuses.sort();
    assert_eq!(statuses,["already_dismissed","dismissed"]);
    let rows=sqlx::query("SELECT bid_order_id,ask_order_id,score::text,match_reasons::jsonb AS reasons,status,recipient_org_id FROM match_suggestions").fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(rows.len(),1); let row=&rows[0]; assert_eq!(row.get::<Uuid,_>("bid_order_id"),id(201)); assert_eq!(row.get::<Uuid,_>("ask_order_id"),id(201)); assert_eq!(row.get::<String,_>("score"),"0"); assert_eq!(row.get::<Value,_>("reasons"),json!([])); assert_eq!(row.get::<String,_>("status"),"DISMISSED"); assert_eq!(row.get::<Uuid,_>("recipient_org_id"),id(10));
    sqlx::query(section("duplicate")).bind(Uuid::new_v4()).bind(id(201)).bind(id(10)).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(id(1),"PATCH",&path).await,(200,json!({"status":"already_dismissed"})));
    assert_eq!(f.request(id(5),"PATCH",&path).await,(200,json!({"status":"dismissed"})));
    assert_eq!(f.request(id(1),"PATCH",&dismiss(999)).await.0,404);
    sqlx::raw_sql(section("abort")).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(id(1),"PATCH",&dismiss(202)).await.0,500);
    let failed:i64=sqlx::query_scalar("SELECT count(*) FROM match_suggestions WHERE bid_order_id=$1").bind(id(202)).fetch_one(&mut f.admin).await.unwrap(); assert_eq!(failed,0);
    sqlx::raw_sql(section("resume")).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.state().await,before);
    f.finish().await;
}

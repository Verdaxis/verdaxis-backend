#![cfg(feature = "contract-tests")]

use std::sync::Arc;

use axum::{Router, body::Body, http::{Request, StatusCode}};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{auth::VerificationSettings, market_support::{SupportSettings, SupportState, router}};

struct Fixture {
    admin: PgConnection,
    pool: sqlx::PgPool,
    actor: Uuid,
    target: Uuid,
    organization: Uuid,
    other: Uuid,
    key: String,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_SUPPORT_ADMIN_DATABASE_URL").expect("explicit disposable administrator URL");
        let runtime_url = std::env::var("RUST_SUPPORT_DATABASE_URL").expect("explicit disposable runtime URL");
        let admin_endpoint = url::Url::parse(&admin_url).unwrap();
        let runtime_endpoint = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_endpoint.host_str(), Some("127.0.0.1"));
        assert!(admin_endpoint.path().ends_with("_test"));
        assert_eq!(admin_endpoint.port(), runtime_endpoint.port());
        assert_eq!(admin_endpoint.path(), runtime_endpoint.path());
        assert_ne!(admin_endpoint.username(), runtime_endpoint.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_support.sql")).execute(&mut admin).await.unwrap();
        let actor = Uuid::new_v4();
        let target = Uuid::new_v4();
        let organization = Uuid::new_v4();
        let other = Uuid::new_v4();
        sqlx::query("SELECT support_fixture_seed($1,$2,$3,$4)").bind(actor).bind(target).bind(organization).bind(other).execute(&mut admin).await.unwrap();
        let pool = PgPoolOptions::new().max_connections(5).connect(&runtime_url).await.unwrap();
        Self { admin, pool, actor, target, organization, other, key: Uuid::new_v4().to_string() }
    }
    fn app(&self, enabled: bool, bootstrap: bool) -> Router {
        let verification = Arc::new(VerificationSettings::new(self.key.clone(), None, "support-tests".into(), "support-tests".into()));
        let policy = serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let config = serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":4,"python_budget":0,"configured_max_connections":100})).unwrap();
        router(SupportState::new(self.pool.clone(), verification, SupportSettings::new(enabled, if bootstrap { &self.actor.to_string() } else { "" }), policy, config))
    }
    async fn request(&self, app: &Router, actor: Uuid, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let now = Utc::now();
        let claims = json!({"sub":actor,"type":"access","iat":now.timestamp(),"exp":(now+Duration::hours(1)).timestamp(),"iss":"support-tests","aud":"support-tests"});
        let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(self.key.as_bytes())).unwrap();
        let response = app.clone().oneshot(Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}")).header("content-type", "application/json").header("x-request-id", "support-request").body(Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    fn grant(&self, capability: &str) -> Value { json!({"user_id":self.target,"capability":capability,"reason":"Support request"}) }
    async fn counts(&mut self) -> (i64,i64,i64) {
        let row = sqlx::query("SELECT (SELECT count(*) FROM audit_logs) AS audit,(SELECT count(*) FROM notifications) AS notifications,(SELECT count(*) FROM market_event_outbox) AS outbox").fetch_one(&mut self.admin).await.unwrap();
        (row.get("audit"),row.get("notifications"),row.get("outbox"))
    }
}
const ROOT: &str = "/api/admin/market-support";

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn retired_routes_return_410_or_404() {
    let mut f = Fixture::new().await;
    let before = f.counts().await;
    for enabled in [false,true] {
        let app = f.app(enabled,true);
        for suffix in [format!("/organizations/{}/authorizations",f.organization),format!("/organizations/{}/listings",f.organization),format!("/organizations/{}/listings/{}/cancel",f.organization,Uuid::new_v4()),format!("/organizations/{}/authorizations/{}/revoke",f.organization,Uuid::new_v4())] {
            let (status, body) = f.request(&app,f.actor,"POST",&format!("{ROOT}{suffix}"),Some(json!({"retired":"not parsed"}))).await;
            assert_eq!(status,if enabled {StatusCode::GONE} else {StatusCode::NOT_FOUND});
            if enabled { assert_eq!(body["detail"]["code"],"MARKET_SUPPORT_LEGACY_MUTATION_RETIRED"); }
        }
    }
    assert_eq!(f.counts().await,before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn support_capability_lifecycle() {
    let mut f = Fixture::new().await;
    let app = f.app(true,true);
    let (status,catalogue) = f.request(&app,f.target,"GET",&format!("{ROOT}/capabilities"),None).await;
    assert_eq!(status,StatusCode::OK);
    assert_eq!(catalogue,json!([]));
    for organization in [f.organization,Uuid::new_v4()] {
        let (status,body) = f.request(&app,f.target,"GET",&format!("{ROOT}/organizations/{organization}/listings"),None).await;
        assert_eq!(status,StatusCode::FORBIDDEN);
        assert_eq!(body["detail"]["code"],"MARKET_SUPPORT_CAPABILITY_REQUIRED");
    }
    let before = f.counts().await;
    let (status,assignment) = f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(f.grant("MARKET_SUPPORT_LISTINGS"))).await;
    assert_eq!(status,StatusCode::CREATED);
    assert_eq!(assignment["revoked_at"],Value::Null);
    let id = assignment["id"].as_str().unwrap();
    let (status, _) = f.request(&app,f.target,"GET",&format!("{ROOT}/organizations/{}/listings",f.organization),None).await;
    assert_eq!(status,StatusCode::OK);
    let (status,revoked) = f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments/{id}/revoke"),Some(json!({"reason":"Request complete"}))).await;
    assert_eq!(status,StatusCode::OK);
    assert_eq!(revoked["revoked_by_user_id"],json!(f.actor));
    assert_eq!(f.request(&app,f.target,"GET",&format!("{ROOT}/capabilities"),None).await.1,json!([]));
    let (_,renewed) = f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(f.grant("MARKET_SUPPORT_LISTINGS"))).await;
    assert_eq!(renewed["id"],assignment["id"]);
    assert_eq!(renewed["revoked_at"],Value::Null);
    assert_eq!(renewed["revocation_reason"],Value::Null);
    let after = f.counts().await;
    assert_eq!(after,(before.0+3,before.1,before.2));
    let audits = sqlx::query("SELECT action,changes,request_id FROM audit_logs WHERE resource_id=$1 ORDER BY timestamp,id").bind(id).fetch_all(&mut f.admin).await.unwrap();
    assert_eq!(audits.len(),3);
    assert!(audits.iter().all(|r|r.get::<Option<String>,_>("request_id").as_deref()==Some("support-request")));
    assert!(audits.iter().any(|r|r.get::<String,_>("action")=="market_support.capability_revoked"));
    assert!(audits.iter().all(|r|r.get::<Value,_>("changes")["target_user_id"]==json!(f.target)));
    sqlx::query("UPDATE staff_capability_assignments SET expires_at=now()-interval '1 second' WHERE id=$1::uuid").bind(id).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(&app,f.target,"GET",&format!("{ROOT}/capabilities"),None).await.1,json!([]));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn bootstrap_allowlist_is_required() {
    let mut f = Fixture::new().await;
    let app = f.app(true,false);
    let before = f.counts().await;
    for user in [f.target,Uuid::new_v4()] {
        let mut body = f.grant("MARKET_SUPPORT_LISTINGS"); body["user_id"]=json!(user);
        assert_eq!(f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(body)).await.0,StatusCode::FORBIDDEN);
    }
    assert_eq!(f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments/{}/revoke",Uuid::new_v4()),Some(json!({"reason":"Request complete"}))).await.0,StatusCode::FORBIDDEN);
    assert_eq!(f.counts().await,before);
    let app = f.app(true,true);
    sqlx::query("UPDATE users SET role='SUPPLIER' WHERE id=$1").bind(f.target).execute(&mut f.admin).await.unwrap();
    assert_eq!(f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(f.grant("MARKET_SUPPORT_LISTINGS"))).await.0,StatusCode::CONFLICT);
    assert_eq!(f.request(&app,f.target,"GET",&format!("{ROOT}/capabilities"),None).await.0,StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn revoke_ends_contexts_in_one_transaction() {
    let mut f = Fixture::new().await;
    let app = f.app(true,true);
    let (_,assignment) = f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(f.grant("MARKET_SUPPORT_LISTINGS"))).await;
    let assignment_id: Uuid = assignment["id"].as_str().unwrap().parse().unwrap();
    let context = Uuid::new_v4();
    let unrelated = Uuid::new_v4();
    sqlx::query("SELECT support_fixture_context($1,$2,$3,$4)").bind(context).bind(f.target).bind(f.organization).bind("ACTIVE").execute(&mut f.admin).await.unwrap();
    sqlx::query("SELECT support_fixture_context($1,$2,$3,$4)").bind(unrelated).bind(f.actor).bind(f.organization).bind("ACTIVE").execute(&mut f.admin).await.unwrap();
    let before = f.counts().await;
    let mut holder = f.pool.begin().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *holder).await.unwrap();
    sqlx::query("SELECT id FROM market_support_contexts WHERE id=$1 FOR UPDATE").bind(context).execute(&mut *holder).await.unwrap();
    let request = f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments/{assignment_id}/revoke"),Some(json!({"reason":"Request complete"})));
    let observer = async {
        tokio::time::timeout(std::time::Duration::from_secs(10),async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid<>$1 AND $1=ANY(pg_blocking_pids(pid)))").bind(holder_pid).fetch_one(&f.pool).await.unwrap();
                if blocked {break;}
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        let unchanged: bool = sqlx::query_scalar("SELECT revoked_at IS NULL FROM staff_capability_assignments WHERE id=$1").bind(assignment_id).fetch_one(&f.pool).await.unwrap();
        assert!(unchanged);
        holder.rollback().await.unwrap();
    };
    let (response,()) = tokio::join!(request,observer);
    assert_eq!(response.0,StatusCode::OK);
    let row = sqlx::query("SELECT status,version,ended_at IS NOT NULL AS ended FROM market_support_contexts WHERE id=$1").bind(context).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String,_>("status"),"REVOKED");
    assert_eq!(row.get::<i32,_>("version"),2);
    assert!(row.get::<bool,_>("ended"));
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM market_support_contexts WHERE id=$1").bind(unrelated).fetch_one(&mut f.admin).await.unwrap(),"ACTIVE");
    assert_eq!(f.counts().await,(before.0+2,before.1,before.2));
    let changes: Value = sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='market_support.context_revoked' AND resource_id=$1").bind(context.to_string()).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(changes,json!({"status":"REVOKED","reason":"Request complete","capability_assignment_id":assignment_id}));
    let before = f.counts().await;
    assert_eq!(f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments/{assignment_id}/revoke"),Some(json!({"reason":"Repeat request"}))).await.0,StatusCode::OK);
    assert_eq!(f.counts().await,(before.0+1,before.1,before.2));
    // A failed audit must roll back both the assignment and its context.
    let (_,_) = f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(f.grant("MARKET_SUPPORT_LISTINGS"))).await;
    sqlx::query("UPDATE market_support_contexts SET status='ACTIVE',ended_at=NULL,version=1 WHERE id=$1").bind(context).execute(&mut f.admin).await.unwrap();
    sqlx::query("SELECT support_fixture_audit_failure(true)").execute(&mut f.admin).await.unwrap();
    let before = f.counts().await;
    assert_eq!(f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments/{assignment_id}/revoke"),Some(json!({"reason":"Request complete"}))).await.0,StatusCode::INTERNAL_SERVER_ERROR);
    sqlx::query("SELECT support_fixture_audit_failure(false)").execute(&mut f.admin).await.unwrap();
    assert!(sqlx::query_scalar::<_,bool>("SELECT revoked_at IS NULL FROM staff_capability_assignments WHERE id=$1").bind(assignment_id).fetch_one(&mut f.admin).await.unwrap());
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM market_support_contexts WHERE id=$1").bind(context).fetch_one(&mut f.admin).await.unwrap(),"ACTIVE");
    assert_eq!(f.counts().await,before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn support_pages_are_bounded_and_real_only() {
    let mut f = Fixture::new().await;
    let app = f.app(true,true);
    for capability in ["MARKET_SUPPORT_LISTINGS","MARKET_SUPPORT_AUTHORIZATIONS"] {
        assert_eq!(f.request(&app,f.actor,"POST",&format!("{ROOT}/capability-assignments"),Some(f.grant(capability))).await.0,StatusCode::CREATED);
    }
    let (status,page) = f.request(&app,f.target,"GET",&format!("{ROOT}/organizations?limit=1"),None).await;
    assert_eq!(status,StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(),1);
    assert_eq!(page["total"],1);
    assert_eq!(page["items"][0]["id"],json!(f.organization));
    assert_eq!(page["items"][0]["domain"],Value::Null);
    for suffix in ["organizations?limit=101","organizations?skip=-1"] {
        assert_eq!(f.request(&app,f.target,"GET",&format!("{ROOT}/{suffix}"),None).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    }
    for suffix in ["authorizations","listings"] {
        assert_eq!(f.request(&app,f.target,"GET",&format!("{ROOT}/organizations/{}/{suffix}",f.other),None).await.0,StatusCode::CONFLICT);
        let (status,page) = f.request(&app,f.target,"GET",&format!("{ROOT}/organizations/{}/{suffix}?limit=1",f.organization),None).await;
        assert_eq!(status,StatusCode::OK);
        assert_eq!(page["items"],json!([]));
        assert_eq!(page["total"],0);
        assert_eq!(page["limit"],1);
    }
}

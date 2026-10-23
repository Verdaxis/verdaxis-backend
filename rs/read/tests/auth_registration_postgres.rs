//! E5/E6 PostgreSQL component checks.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/registration.rs"]
pub mod registration;
use axum::{Router, Json, body::{Body,to_bytes}, http::{Request,StatusCode}, routing::post};
use serde_json::{Value,json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tower::ServiceExt;
use uuid::Uuid;
use registration::{RegistrationSettings,router};

struct Fixture {
    owner: PgPool,
    settings: RegistrationSettings,
    mail: Arc<Mutex<Vec<Value>>>,
    release: Arc<Semaphore>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(fail: bool) -> Self {
        let owner = PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap()).await.unwrap();
        let pool = PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap()).await.unwrap();
        let mail = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Semaphore::new(0));
        let received = mail.clone();
        let gate = release.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/", post(move |Json(payload): Json<Value>| {
            let received = received.clone(); let gate = gate.clone();
            async move {
                received.lock().await.push(payload);
                gate.acquire().await.unwrap().forget();
                if fail { StatusCode::BAD_GATEWAY } else { StatusCode::OK }
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
        let dispatcher = email::MailDispatcher::new(crud::referrals::invite::MailClient::loopback(format!("http://{addr}/"),"synthetic".into()).unwrap());
        Self { owner, settings: RegistrationSettings::new(pool,dispatcher).with_domain_lists(["gmail.com".into()], Vec::<String>::new()), mail, release, server }
    }
    async fn request(&self,path: &str,body: Value) -> (StatusCode,axum::http::HeaderMap,Value) {
        request(router(self.settings.clone()),path,body).await
    }
    async fn drain(&self) {
        self.release.add_permits(100);
        tokio::time::timeout(std::time::Duration::from_secs(20), self.settings.mail.drain()).await.unwrap();
    }
    async fn token(&self, email: &str) -> String {
        self.drain().await;
        let mails = self.mail.lock().await;
        let html = mails.iter().rev().find(|mail| mail["to"][0]==email).unwrap()["html"].as_str().unwrap();
        let token = html.split("token=").nth(1).unwrap().split('"').next().unwrap();
        token.to_string()
    }
    async fn stop(self) { self.drain().await; self.server.abort(); let _ = self.server.await; }
}
async fn request(app: Router,path: &str,body: Value) -> (StatusCode,axum::http::HeaderMap,Value) {
    let response = tokio::time::timeout(std::time::Duration::from_secs(15),app.oneshot(Request::post(path).header("content-type","application/json").body(Body::from(body.to_string())).unwrap())).await.unwrap().unwrap();
    let status=response.status(); let headers=response.headers().clone();
    let body=serde_json::from_slice(&to_bytes(response.into_body(),65536).await.unwrap()).unwrap();
    (status,headers,body)
}
fn signup(email: &str, referral: Option<&str>) -> Value {
    json!({"email":email,"password":"valid-registration-password","role":"BUYER","first_name":"Test","last_name":"Account","referral_code":referral})
}
fn organization(token: &str) -> Value {
    json!({"registration_token":token,"organization":{"name":"Synthetic Company","type":"FUEL_BUYER","country_code":"SG"}})
}
async fn count(pool: &PgPool,sql: &str,email: &str) -> i64 {
    sqlx::query_scalar(sql).bind(email).fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_response_and_delivery_boundary() {
    let f=Fixture::new(false).await;
    let domain=format!("{}.example",Uuid::new_v4());
    let org=Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,name,type,domain) VALUES($1,'Synthetic','FUEL_BUYER',$2)").bind(org).bind(&domain).execute(&f.owner).await.unwrap();
    let mut responses=Vec::new();
    for (index,verified) in [None,Some(true),Some(false),None].into_iter().enumerate() {
        let email=if index==3 {format!("new@{}.example",Uuid::new_v4())} else {format!("user{index}@{domain}")};
        if let Some(verified)=verified {
            sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified) VALUES($1,$2,'unchanged','PENDING',$3)").bind(Uuid::new_v4()).bind(&email).bind(verified).execute(&f.owner).await.unwrap();
        }
        let response=f.request("/api/auth/register",signup(&email,None)).await;
        assert_eq!(response.0,StatusCode::ACCEPTED);
        assert_eq!(response.2,json!({"status":"check_email"}));
        assert!(!response.1.contains_key("set-cookie"));
        responses.push((response.0,response.1,response.2));
        if verified.is_some() {
            assert_eq!(count(&f.owner,"SELECT count(*) FROM users WHERE email=$1 AND password_hash='unchanged'",&email).await,1);
            assert_eq!(count(&f.owner,"SELECT count(*) FROM pending_registrations WHERE email=$1",&email).await,0);
            assert_eq!(count(&f.owner,"SELECT count(*) FROM audit_logs a JOIN users u ON a.user_id=u.id WHERE u.email=$1",&email).await,0);
        }
    }
    assert!(responses.windows(2).all(|pair| pair[0]==pair[1]));
    // Provider completion requires permits that no request has released.
    assert_eq!(f.release.available_permits(),0);
    f.drain().await;
    assert_eq!(f.mail.lock().await.len(),4);
    let joined: (Option<Uuid>,String,i64) = sqlx::query_as("SELECT u.organization_id,u.status::text,(SELECT count(*) FROM organization_join_requests j WHERE j.user_id=u.id AND j.organization_id=$2) FROM users u WHERE email=$1").bind(format!("user0@{domain}")).bind(org).fetch_one(&f.owner).await.unwrap();
    assert_eq!(joined,(None,"PENDING".into(),1));
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e6_pending_verification_and_single_consumption() {
    let f=Fixture::new(false).await;
    let email=format!("{}@gmail.com",Uuid::new_v4());
    assert_eq!(f.request("/api/auth/register",signup(&email,None)).await.0,StatusCode::ACCEPTED);
    let token=f.token(&email).await;
    let verify_settings=email_verification::EmailSettings::new(f.settings.pool.clone(),f.settings.mail.clone());
    let response=email_verification::router(verify_settings).oneshot(Request::post(format!("/api/auth/verify-email?token={token}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(),StatusCode::OK);
    let body: Value=serde_json::from_slice(&to_bytes(response.into_body(),4096).await.unwrap()).unwrap();
    assert_eq!(body["requires_org"],true); assert_eq!(body["registration_token"],token); assert_eq!(body["email"],email);
    let result=f.request("/api/auth/register-with-org",organization(&token)).await;
    assert_eq!(result.0,StatusCode::OK);
    assert_eq!(result.2["email"],email); assert_eq!(result.2["status"],"PENDING"); assert!(result.2["organization_id"].is_null());
    let replay=f.request("/api/auth/register-with-org",organization(&token)).await;
    assert_eq!(replay.0,StatusCode::BAD_REQUEST); assert_eq!(replay.2["detail"]["code"],"REGISTRATION_TOKEN_INVALID");
    assert_eq!(count(&f.owner,"SELECT count(*) FROM pending_registrations WHERE email=$1 AND used_at IS NOT NULL",&email).await,1);
    assert_eq!(count(&f.owner,"SELECT count(*) FROM organization_join_requests j JOIN users u ON j.user_id=u.id WHERE u.email=$1",&email).await,1);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_e6_atomic_referral_registration() {
    let f=Fixture::new(false).await;
    let domain=format!("{}.example",Uuid::new_v4());
    let code=format!("G{}",&Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query("INSERT INTO users(id,email,password_hash,status,referral_code) VALUES($1,$2,'unused','PENDING',$3)").bind(Uuid::new_v4()).bind(format!("referrer@{domain}")).bind(&code).execute(&f.owner).await.unwrap();
    sqlx::query("INSERT INTO organizations(id,name,type,domain) VALUES($1,'Synthetic','FUEL_BUYER',$2)").bind(Uuid::new_v4()).bind(&domain).execute(&f.owner).await.unwrap();
    let email=format!("rollback@{domain}");
    // An INSERT after attribution fails, so every earlier write must roll back.
    sqlx::query("CREATE OR REPLACE FUNCTION registration_test_reject_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.changes->>'email' LIKE 'rollback@%' THEN RAISE EXCEPTION 'fixture rejection'; END IF; RETURN NEW; END $$").execute(&f.owner).await.unwrap();
    sqlx::query("CREATE TRIGGER registration_test_reject_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION registration_test_reject_audit()").execute(&f.owner).await.unwrap();
    let result=f.request("/api/auth/register",signup(&email,Some(&code))).await;
    sqlx::query("DROP TRIGGER registration_test_reject_audit ON audit_logs").execute(&f.owner).await.unwrap();
    assert_eq!(result.0,StatusCode::SERVICE_UNAVAILABLE);
    for sql in ["SELECT count(*) FROM users WHERE email=$1","SELECT count(*) FROM referrals r JOIN users u ON r.referred_user_id=u.id WHERE u.email=$1","SELECT count(*) FROM organization_join_requests j JOIN users u ON j.user_id=u.id WHERE u.email=$1","SELECT count(*) FROM user_status_transitions s JOIN users u ON s.user_id=u.id WHERE u.email=$1","SELECT count(*) FROM audit_logs WHERE changes->>'email'=$1"] {
        assert_eq!(count(&f.owner,sql,&email).await,0);
    }
    assert!(f.mail.lock().await.is_empty());
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_duplicate_registration_counts_referral_once() {
    let f=Fixture::new(false).await;
    let domain=format!("{}.example",Uuid::new_v4());
    let code=format!("G{}",&Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query("INSERT INTO users(id,email,password_hash,status,referral_code) VALUES($1,$2,'unused','PENDING',$3)").bind(Uuid::new_v4()).bind(format!("referrer@{domain}")).bind(&code).execute(&f.owner).await.unwrap();
    sqlx::query("INSERT INTO organizations(id,name,type,domain) VALUES($1,'Synthetic','FUEL_BUYER',$2)").bind(Uuid::new_v4()).bind(&domain).execute(&f.owner).await.unwrap();
    let email=format!("duplicate@{domain}");
    let barrier=Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks=Vec::new();
    for _ in 0..2 {
        let app=router(f.settings.clone()); let body=signup(&email,Some(&code)); let barrier=barrier.clone();
        tasks.push(tokio::spawn(async move {barrier.wait().await; request(app,"/api/auth/register",body).await}));
    }
    barrier.wait().await;
    for task in tasks {assert_eq!(task.await.unwrap().0,StatusCode::ACCEPTED);}
    assert_eq!(count(&f.owner,"SELECT count(*) FROM users WHERE email=$1",&email).await,1);
    assert_eq!(count(&f.owner,"SELECT count(*) FROM referrals r JOIN users u ON r.referred_user_id=u.id WHERE u.email=$1",&email).await,1);
    assert_eq!(count(&f.owner,"SELECT count(*) FROM user_status_transitions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",&email).await,1);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_failed_delivery_preserves_response() {
    let f=Fixture::new(true).await;
    let email=format!("{}@gmail.com",Uuid::new_v4());
    let response=f.request("/api/auth/register",signup(&email,None)).await;
    assert_eq!(response.0,StatusCode::ACCEPTED); assert_eq!(response.2,json!({"status":"check_email"}));
    f.drain().await;
    assert_eq!(f.mail.lock().await.len(),3);
    f.stop().await;
}

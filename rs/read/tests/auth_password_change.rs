pub use verdaxis_read::{ApiError,auth,runtime};
#[path="../src/auth/password_change.rs"]
mod password_change;
#[path="../src/auth/logout.rs"]
mod logout;
use axum::{body::{Body,to_bytes},http::Request};
use sqlx::{PgPool,postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
const SECRET:&str="synthetic-session-key-at-least-32-characters";
fn state(pool:PgPool)->password_change::PasswordChangeState {password_change::PasswordChangeState::new(pool,"production",SECRET,None,"verdaxis","verdaxis-api",auth::limits::SharedLimits::default())}
fn request(token:&str,current:&str,origin:&str)->Request<Body>{Request::put("/api/auth/me/password").header("origin",origin).header("authorization",format!("Bearer {token}")).header("content-type","application/json").body(Body::from(serde_json::json!({"current_password":current,"new_password":"NewSyntheticPassword123"}).to_string())).unwrap()}
#[tokio::test]
async fn password_change_requires_trusted_origin_and_quota(){
 let pool=PgPoolOptions::new().connect_lazy("postgres://unused:unused@127.0.0.1:1/unused").unwrap();let state=state(pool);let token=auth::TokenIssuer::new(SECRET,"verdaxis","verdaxis-api").access(Uuid::new_v4(),chrono::Utc::now(),None);
 let response=password_change::router(state.clone()).oneshot(request(&token,"old","https://other.invalid")).await.unwrap();assert_eq!(response.status(),403);
 let now=chrono::Utc::now().timestamp() as u64;for _ in 0..3{state.limits.check_endpoint("unknown","/api/auth/me/password",now).unwrap();}
 let response=password_change::router(state).oneshot(request(&token,"old","https://app.verdaxis.exchange")).await.unwrap();assert_eq!(response.status(),429);
}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn password_change_and_logout_session_contract(){
 let owner=PgPoolOptions::new().connect(&std::env::var("AUTH_STORE_TEST_OWNER_URL").unwrap()).await.unwrap();let pool=PgPoolOptions::new().connect(&std::env::var("AUTH_STORE_TEST_DATABASE_URL").unwrap()).await.unwrap();let id=Uuid::new_v4();let now=chrono::Utc::now();let old=auth::password::hash("OldSyntheticPassword123").await.unwrap();
 sqlx::query("INSERT INTO users (id,email,password_hash,role,status,email_verified,must_change_password,password_changed_at) VALUES ($1,$2,$3,'BUYER','APPROVED',true,true,$4)").bind(id).bind(format!("{id}@task7.invalid")).bind(&old).bind(now-chrono::Duration::days(1)).execute(&owner).await.unwrap();
 for _ in 0..2{auth::store::insert_refresh(&mut owner.acquire().await.unwrap(),&auth::store::NewRefreshRow{user_id:id,family_id:Uuid::new_v4(),jti_hash:auth::token_bytes::token_digest(&auth::token_bytes::generate_token()),device_id_hash:auth::token_bytes::token_digest(&auth::token_bytes::generate_token()),expires_at:now+chrono::Duration::days(7)}).await.unwrap();}
 let token=auth::TokenIssuer::new(SECRET,"verdaxis","verdaxis-api").access(id,now,None);let state=state(pool.clone());
 let response=password_change::router(state.clone()).oneshot(request(&token,"wrong","https://app.verdaxis.exchange")).await.unwrap();assert_eq!(response.status(),400);assert_eq!(sqlx::query_scalar::<_,String>("SELECT password_hash FROM users WHERE id=$1").bind(id).fetch_one(&owner).await.unwrap(),old);
 let response=password_change::router(state).oneshot(request(&token,"OldSyntheticPassword123","https://app.verdaxis.exchange")).await.unwrap();assert_eq!(response.status(),200);
 let cookies:Vec<_>=response.headers().get_all("set-cookie").iter().map(|v|v.to_str().unwrap().split(';').next().unwrap().to_owned()).collect();assert_eq!(cookies.len(),2);
 let body:serde_json::Value=serde_json::from_slice(&to_bytes(response.into_body(),8192).await.unwrap()).unwrap();
 let verification=auth::VerificationSettings::new(SECRET.into(),None,"verdaxis".into(),"verdaxis-api".into());let access=auth::verify_access(body["access_token"].as_str().unwrap(),&verification,chrono::Utc::now()).unwrap();
 let changed:chrono::DateTime<chrono::Utc>=sqlx::query_scalar("SELECT password_changed_at FROM users WHERE id=$1").bind(id).fetch_one(&owner).await.unwrap();assert!(changed>=now-chrono::Duration::days(1));assert!(access.issued_at.unwrap()>changed);
 assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(id).fetch_one(&owner).await.unwrap(),1);
 let hash:String=sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1").bind(id).fetch_one(&owner).await.unwrap();assert!(auth::password::verify("NewSyntheticPassword123",&hash).await);
 let response=logout::router(logout::LogoutState::new(pool,"production",SECRET,None,"verdaxis","verdaxis-api")).oneshot(Request::post("/api/auth/logout").header("origin","https://app.verdaxis.exchange").header("cookie",cookies.join("; ")).body(Body::empty()).unwrap()).await.unwrap();assert_eq!(response.status(),204);assert_eq!(response.headers().get_all("set-cookie").iter().count(),2);
 assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(id).fetch_one(&owner).await.unwrap(),0);
 sqlx::query("DELETE FROM audit_logs WHERE user_id=$1").bind(id).execute(&owner).await.unwrap();sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1").bind(id).execute(&owner).await.unwrap();sqlx::query("DELETE FROM users WHERE id=$1").bind(id).execute(&owner).await.unwrap();
}

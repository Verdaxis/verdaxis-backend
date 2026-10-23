#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, auth, runtime};
#[allow(dead_code)]
#[path = "../src/auth/refresh.rs"]
mod refresh;
use axum::{body::{Body,to_bytes},http::{Request,StatusCode},response::Response};
use chrono::{Utc,Duration};
use sqlx::{PgPool,postgres::PgPoolOptions,Row};
use uuid::Uuid;
use tower::ServiceExt;
use verdaxis_read::auth::{TokenIssuer,limits::SharedLimits,token_bytes::{generate_token,token_digest},store::{insert_refresh,NewRefreshRow}};
const SECRET:&str="synthetic-refresh-key-at-least-32-characters";
struct Fixture { owner:PgPool,state:refresh::RefreshState,id:Uuid,family:Uuid,jti:String,device:String,token:String }
impl Fixture {
 async fn new()->Self {
  let owner=PgPoolOptions::new().max_connections(4).connect(&std::env::var("AUTH_REFRESH_OWNER_URL").expect("owner URL")).await.unwrap();
  let pool=PgPoolOptions::new().max_connections(4).connect(&std::env::var("AUTH_REFRESH_DATABASE_URL").expect("runtime URL")).await.unwrap();
  let id=Uuid::new_v4();let family=Uuid::new_v4();let jti=generate_token();let device=generate_token();let now=Utc::now();
  sqlx::query("INSERT INTO users (id,email,password_hash,role,status,email_verified,must_change_password) VALUES ($1,$2,'unused','BUYER','APPROVED',true,false)").bind(id).bind(format!("{id}@refresh.invalid")).execute(&owner).await.unwrap();
  insert_refresh(&mut *owner.acquire().await.unwrap(),&NewRefreshRow{user_id:id,family_id:family,jti_hash:token_digest(&jti),device_id_hash:token_digest(&device),expires_at:now+Duration::days(7)}).await.unwrap();
  let token=TokenIssuer::new(SECRET,"verdaxis","verdaxis-api").refresh(id,family,&jti,now,None);
  let state=refresh::RefreshState::new(pool,"production",SECRET,None,"verdaxis","verdaxis-api",SharedLimits::default());
  Self{owner,state,id,family,jti,device,token}
 }
 async fn send(&self,token:&str,device:&str)->Response {send(self.state.clone(),token,device).await}
 async fn rows(&self)->serde_json::Value {sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY id),'[]') FROM refresh_sessions s WHERE user_id=$1").bind(self.id).fetch_one(&self.owner).await.unwrap()}
 async fn cleanup(&self){sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1").bind(self.id).execute(&self.owner).await.unwrap();sqlx::query("DELETE FROM users WHERE id=$1").bind(self.id).execute(&self.owner).await.unwrap();}
}
async fn send(state:refresh::RefreshState,token:&str,device:&str)->Response {refresh::router(state).oneshot(Request::post("/api/auth/refresh").header("origin","https://app.verdaxis.exchange").header("cookie",format!("refresh_token={token}; device_session={device}")).body(Body::empty()).unwrap()).await.unwrap()}
async fn response_code(response:Response)->String {let body:serde_json::Value=serde_json::from_slice(&to_bytes(response.into_body(),8192).await.unwrap()).unwrap();body["detail"]["code"].as_str().unwrap().to_owned()}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn refresh_rotates_and_keeps_family(){
 let f=Fixture::new().await;let response=f.send(&f.token,&f.device).await;assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers().get_all("set-cookie").iter().count(),2);
 let body:serde_json::Value=serde_json::from_slice(&to_bytes(response.into_body(),8192).await.unwrap()).unwrap();assert_eq!(body.as_object().unwrap().len(),2);assert_eq!(body["token_type"],"bearer");
 let old=sqlx::query("SELECT revoked,replaced_by_jti_hash,rotation_grace_until,last_used_at FROM refresh_sessions WHERE jti_hash=$1").bind(token_digest(&f.jti)).fetch_one(&f.owner).await.unwrap();assert!(old.get::<bool,_>("revoked"));let next=old.get::<String,_>("replaced_by_jti_hash");assert!(old.get::<Option<chrono::DateTime<Utc>>,_>("last_used_at").is_some());assert!(old.get::<Option<chrono::DateTime<Utc>>,_>("rotation_grace_until").is_some());
 let live=sqlx::query("SELECT family_id,device_id_hash,jti_hash FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(f.id).fetch_all(&f.owner).await.unwrap();assert_eq!(live.len(),1);assert_eq!(live[0].get::<Uuid,_>("family_id"),f.family);assert_eq!(live[0].get::<String,_>("device_id_hash"),token_digest(&f.device));assert_eq!(live[0].get::<String,_>("jti_hash"),next);f.cleanup().await;
}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn refresh_duplicate_within_grace_is_409(){let f=Fixture::new().await;assert_eq!(f.send(&f.token,&f.device).await.status(),200);let before=f.rows().await;let response=f.send(&f.token,&f.device).await;assert_eq!(response.status(),409);assert_eq!(response.headers()["retry-after"],"1");assert_eq!(response_code(response).await,"REFRESH_ROTATION_IN_PROGRESS");assert_eq!(f.rows().await,before);f.cleanup().await;}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn refresh_replay_after_grace_revokes_family(){let f=Fixture::new().await;assert_eq!(f.send(&f.token,&f.device).await.status(),200);sqlx::query("UPDATE refresh_sessions SET rotation_grace_until=$2 WHERE jti_hash=$1").bind(token_digest(&f.jti)).bind(Utc::now()-Duration::seconds(1)).execute(&f.owner).await.unwrap();assert_eq!(response_code(f.send(&f.token,&f.device).await).await,"REFRESH_TOKEN_REPLAYED");assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(f.id).fetch_one(&f.owner).await.unwrap(),0);f.cleanup().await;}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn refresh_binding_and_account_vectors(){let f=Fixture::new().await;let before=f.rows().await;assert_eq!(response_code(f.send(&f.token,&generate_token()).await).await,"REFRESH_SESSION_REVOKED");assert_eq!(f.rows().await,before);sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1").bind(f.id).execute(&f.owner).await.unwrap();assert_eq!(response_code(f.send(&f.token,&f.device).await).await,"REFRESH_ACCOUNT_INACTIVE");assert_eq!(f.rows().await,before);f.cleanup().await;}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn refresh_cutoff_order_password_then_revocation(){let f=Fixture::new().await;let now=Utc::now();sqlx::query("UPDATE users SET password_changed_at=$2,authentication_revoked_at=$2 WHERE id=$1").bind(f.id).bind(now).execute(&f.owner).await.unwrap();let before=f.rows().await;assert_eq!(response_code(f.send(&f.token,&f.device).await).await,"REFRESH_PASSWORD_CHANGED");sqlx::query("UPDATE users SET password_changed_at=NULL WHERE id=$1").bind(f.id).execute(&f.owner).await.unwrap();assert_eq!(response_code(f.send(&f.token,&f.device).await).await,"REFRESH_SESSION_REVOKED");assert_eq!(f.rows().await,before);f.cleanup().await;}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn refresh_issuance_range_vectors(){use jsonwebtoken::{encode,Header,EncodingKey};let f=Fixture::new().await;sqlx::query("UPDATE users SET password_changed_at=$2 WHERE id=$1").bind(f.id).bind(Utc::now()).execute(&f.owner).await.unwrap();for issuance in [serde_json::json!(i64::MAX),serde_json::json!(-1),serde_json::json!(1.5),serde_json::json!("invalid"),serde_json::json!(true)]{let claims=serde_json::json!({"sub":f.id,"family_id":f.family,"jti":f.jti,"type":"refresh","iat":issuance,"exp":Utc::now().timestamp()+60,"iss":"verdaxis","aud":"verdaxis-api"});let token=encode(&Header::default(),&claims,&EncodingKey::from_secret(SECRET.as_bytes())).unwrap();assert_eq!(response_code(f.send(&token,&f.device).await).await,"REFRESH_PASSWORD_CHANGED");}f.cleanup().await;}
#[tokio::test]
#[ignore="requires disposable PostgreSQL"]
async fn concurrent_refresh_same_token_yields_one_successor(){let mut f=Fixture::new().await;let barrier=std::sync::Arc::new(tokio::sync::Barrier::new(3));f.state.hooks.before_device=Some(barrier.clone());let a=tokio::spawn({let state=f.state.clone();let token=f.token.clone();let device=f.device.clone();async move {send(state,&token,&device).await}});let b=tokio::spawn({let state=f.state.clone();let token=f.token.clone();let device=f.device.clone();async move {send(state,&token,&device).await}});barrier.wait().await;let statuses=[a.await.unwrap().status().as_u16(),b.await.unwrap().status().as_u16()];assert!(statuses==[200,409]||statuses==[409,200]);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked").bind(f.id).fetch_one(&f.owner).await.unwrap(),1);f.cleanup().await;}

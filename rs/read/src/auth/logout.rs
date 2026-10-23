//! Device logout; cookie removal is independent of transaction outcome.
use std::sync::Arc;
use axum::{Router,extract::{State,Request},routing::post,response::{Response,IntoResponse},http::StatusCode};
use axum_extra::extract::CookieJar;
use jsonwebtoken::{Algorithm,DecodingKey,Validation,decode};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use crate::{ApiError,auth::{cookies::AuthCookies,error::{AuthCode,AuthFailure},origin::require_trusted_origin,store,token_bytes::token_digest,lock::WriteConflict}};
#[derive(Clone)]
pub struct LogoutState{pool:PgPool,environment:String,keys:Arc<Vec<DecodingKey>>,issuer:String,audience:String}
impl LogoutState{pub fn new(pool:PgPool,environment:&str,secret:&str,previous:Option<&str>,issuer:&str,audience:&str)->Self{Self{pool,environment:environment.into(),keys:Arc::new(std::iter::once(secret).chain(previous).map(|s|DecodingKey::from_secret(s.as_bytes())).collect()),issuer:issuer.into(),audience:audience.into()}}}
pub fn router(state:LogoutState)->Router{Router::new().route("/api/auth/logout",post(logout)).route("/api/auth/logout/",post(logout)).with_state(state)}
async fn logout(State(state):State<LogoutState>,jar:CookieJar,request:Request)->Response{
 let presented=AuthCookies::read(&jar);let cleared=AuthCookies::for_environment(&state.environment).clear(jar);
 let result=async{
 require_trusted_origin(request.headers(),&state.environment,presented.cookie_authenticated()).map_err(|_|ApiError::from(AuthFailure::new(AuthCode::BrowserOriginRejected)))?;
 let family=presented.refresh.as_deref().and_then(|token|legacy_family(&state,token));
 let device=presented.valid_device().map(token_digest);
 if device.is_none()&&family.is_none(){return Ok(());}
 for attempt in 0..2{let result=async{
 let mut tx=state.pool.begin().await?;sqlx::query("SET TRANSACTION READ WRITE").execute(&mut *tx).await?;
 if let Some(device)=&device{store::lock_device(&mut tx,device).await?;store::revoke_device_families(&mut tx,device,family.map(|(_,f)|f)).await?;}
 else if let Some((user,family))=family{store::lock_user(&mut tx,user).await?;sqlx::query("UPDATE refresh_sessions SET revoked=true WHERE user_id=$1 AND family_id=$2").bind(user).bind(family).execute(&mut *tx).await?;}
 tx.commit().await
 }.await;
 match result{Ok(())=>return Ok(()),Err(error) if attempt==0&&error.is_write_conflict()=>continue,Err(_)=>return Err(ApiError{status:StatusCode::SERVICE_UNAVAILABLE,..ApiError::database()})}
 }Err(ApiError::database())
 }.await;
 let response=match result{Ok(())=>StatusCode::NO_CONTENT.into_response(),Err(error)=>error.into_response()};(cleared,response).into_response()
}
fn legacy_family(state:&LogoutState,token:&str)->Option<(Uuid,Uuid)>{let mut validation=Validation::new(Algorithm::HS256);validation.set_issuer(&[&state.issuer]);validation.set_audience(&[&state.audience]);validation.leeway=0;let claims=state.keys.iter().find_map(|key|decode::<Value>(token,key,&validation).ok())?.claims;if claims.get("type")?.as_str()? != "refresh"{return None;}Some((Uuid::parse_str(claims.get("sub")?.as_str()?).ok()?,Uuid::parse_str(claims.get("family_id")?.as_str()?).ok()?))}

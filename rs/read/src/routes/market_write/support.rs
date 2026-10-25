//! Support component routes. Serving composition is a separate owner.
use std::sync::Arc;
use axum::{Router, body::Bytes, extract::{Path, RawQuery, State}, http::{HeaderMap,StatusCode}, response::{IntoResponse,Response}, routing::{get,post}};
use chrono::Utc;
use serde_json::{Value,json};
use tokio::sync::Semaphore;
use uuid::Uuid;
use crate::auth::{VerificationSettings,verify_authorization};
use crate::market_support::{AuditContext,CapabilityGrant,Page,SupportAction,SupportCommand,SupportSettings,reason,run_support};
use crate::{Config,MarketPolicy};

#[derive(Clone)]
pub struct SupportState {
    pool:sqlx::PgPool,
    verification:Arc<VerificationSettings>,
    settings:SupportSettings,
    policy:Arc<MarketPolicy>,
    config:Arc<Config>,
    permits:Arc<Semaphore>,
}
impl SupportState {
    pub fn new(pool:sqlx::PgPool,verification:Arc<VerificationSettings>,settings:SupportSettings,policy:MarketPolicy,config:Config)->Self {
        Self{pool,verification,settings,policy:Arc::new(policy),config:Arc::new(config),permits:Arc::new(Semaphore::new(4))}
    }
}
pub fn router(state:SupportState)->Router {
    Router::new()
        .route("/api/admin/market-support/capabilities",get(capabilities))
        .route("/api/admin/market-support/capability-assignments",post(grant))
        .route("/api/admin/market-support/capability-assignments/{id}/revoke",post(revoke))
        .route("/api/admin/market-support/organizations",get(organizations))
        .route("/api/admin/market-support/organizations/{id}/authorizations",get(authorizations).post(retired))
        .route("/api/admin/market-support/organizations/{id}/listings",get(listings).post(retired))
        .route("/api/admin/market-support/organizations/{id}/listings/{order}/cancel",post(retired))
        .route("/api/admin/market-support/organizations/{id}/authorizations/{authorization}/revoke",post(retired))
        .with_state(state)
}
fn invalid()->Response {(StatusCode::UNPROCESSABLE_ENTITY,axum::Json(json!({"detail":{"code":"VALIDATION_FAILED","message":"The support request is not valid"}}))).into_response()}
async fn execute(state:SupportState,headers:HeaderMap,action:SupportAction)->Response {
    let access=match verify_authorization(headers.get("authorization").and_then(|v|v.to_str().ok()),&state.verification,Utc::now()) {
        Ok(access)=>access,
        Err(error)=>return (StatusCode::from_u16(error.status()).expect("auth status"),axum::Json(json!({"detail":{"code":"AUTH_CREDENTIALS_INVALID","message":"Valid access credentials are required"}}))).into_response(),
    };
    let audit=match AuditContext::new(None,headers.get("x-request-id").and_then(|v|v.to_str().ok()).map(str::to_owned)) {Ok(audit)=>audit,Err(error)=>return error.into_response()};
    let command=SupportCommand{action,settings:state.settings,access:Arc::new(access),policy:state.policy,config:state.config,audit};
    match run_support(state.pool,state.permits,command).await {Ok(value)=>axum::Json(value).into_response(),Err(error)=>error.into_response()}
}
async fn capabilities(State(state):State<SupportState>,headers:HeaderMap)->Response {execute(state,headers,SupportAction::Capabilities).await}
async fn grant(State(state):State<SupportState>,headers:HeaderMap,body:Bytes)->Response {
    let Ok(value)=serde_json::from_slice(&body) else {return invalid();};
    let Ok(grant)=CapabilityGrant::parse(value,Utc::now()) else {return invalid();};
    let mut response=execute(state,headers,SupportAction::Grant(grant)).await;
    if response.status()==StatusCode::OK {*response.status_mut()=StatusCode::CREATED;}
    response
}
async fn revoke(State(state):State<SupportState>,headers:HeaderMap,Path(id):Path<Uuid>,body:Bytes)->Response {
    let Ok(value)=serde_json::from_slice::<Value>(&body) else {return invalid();};
    let Some(raw)=value["reason"].as_str() else {return invalid();};
    let Ok(reason)=reason(raw) else {return invalid();};
    execute(state,headers,SupportAction::Revoke{assignment:id,reason}).await
}
async fn organizations(State(state):State<SupportState>,headers:HeaderMap,RawQuery(raw):RawQuery)->Response {
    let Ok(page)=Page::parse(raw.as_deref()) else {return invalid();};
    execute(state,headers,SupportAction::Organizations(page)).await
}
async fn authorizations(State(state):State<SupportState>,headers:HeaderMap,Path(id):Path<Uuid>,RawQuery(raw):RawQuery)->Response {
    let Ok(page)=Page::parse(raw.as_deref()) else {return invalid();};
    execute(state,headers,SupportAction::Authorizations{organization:id,page}).await
}
async fn listings(State(state):State<SupportState>,headers:HeaderMap,Path(id):Path<Uuid>,RawQuery(raw):RawQuery)->Response {
    let Ok(page)=Page::parse(raw.as_deref()) else {return invalid();};
    execute(state,headers,SupportAction::Listings{organization:id,page}).await
}
async fn retired(State(state):State<SupportState>,headers:HeaderMap)->Response {execute(state,headers,SupportAction::Retired).await}

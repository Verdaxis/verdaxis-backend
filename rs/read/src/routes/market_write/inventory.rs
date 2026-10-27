//! Inventory component routes; serving composition remains separate.
//!
//! The routes pass each request on unchecked: the inventory component checks
//! the query, the record identifier and the body only after authentication
//! and, for a write, admission. Only JSON syntax is read first, as FastAPI
//! parses a JSON body before it resolves the route dependencies.
//!
//! A support-context header does not change these routes. The reads ignore it,
//! and the serving boundary refuses any write that carries one before a route
//! runs, as the Python support-scope middleware does.
use crate::{
    AppState, InventoryAction,
    auth::{VerificationSettings, verify_authorization},
    execute_inventory,
};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::Value;

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: std::sync::Arc<VerificationSettings>,
}

pub fn router(app: AppState, verification: VerificationSettings) -> Router {
    Router::new()
        .route("/api/inventory", get(list).post(create))
        .route(
            "/api/inventory/{item_id}",
            axum::routing::patch(update).delete(delete),
        )
        .route("/api/listings/my", get(listings))
        .with_state(RouteState {
            app,
            verification: std::sync::Arc::new(verification),
        })
}
async fn dispatch(state: RouteState, headers: HeaderMap, action: InventoryAction) -> Response {
    let access = match verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &state.verification,
        chrono::Utc::now(),
    ) {
        Ok(access) => access,
        Err(failure) => {
            return crate::auth::request::RequestError::Authentication(failure).into_response();
        }
    };
    let deleting = matches!(action, InventoryAction::Delete { .. });
    match execute_inventory(state.app, access, action).await {
        Ok(_) if deleting => StatusCode::NO_CONTENT.into_response(),
        Ok(body) => axum::Json(body).into_response(),
        Err(failure) => failure.into_response(),
    }
}
/// Read the body's JSON syntax; `None` means the body is not valid JSON. An
/// empty body, or one declared with another media type, is passed on as
/// `null`; the field rules refuse it after admission.
fn json_body(headers: &HeaderMap, raw: &[u8]) -> Option<Value> {
    let declared_json = headers
        .get(CONTENT_TYPE)
        .is_none_or(|value| value.to_str().is_ok_and(is_json_media_type));
    if raw.is_empty() || !declared_json {
        return Some(Value::Null);
    }
    serde_json::from_slice(raw).ok()
}
fn invalid_json() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        axum::Json(serde_json::json!({"detail":{
            "code":"VALIDATION_FAILED",
            "message":"The request body is not valid JSON"}})),
    )
        .into_response()
}
/// `application/json` or an `application/*+json` type; parameters are ignored.
fn is_json_media_type(value: &str) -> bool {
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence
        .strip_prefix("application/")
        .is_some_and(|subtype| subtype == "json" || subtype.ends_with("+json"))
}
async fn list(
    State(state): State<RouteState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    dispatch(state, headers, InventoryAction::List { query }).await
}
async fn listings(
    State(state): State<RouteState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    dispatch(state, headers, InventoryAction::Listings { query }).await
}
async fn create(State(state): State<RouteState>, headers: HeaderMap, raw: Bytes) -> Response {
    match json_body(&headers, &raw) {
        Some(body) => dispatch(state, headers, InventoryAction::Create { body }).await,
        None => invalid_json(),
    }
}
async fn update(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    raw: Bytes,
) -> Response {
    match json_body(&headers, &raw) {
        Some(body) => dispatch(state, headers, InventoryAction::Patch { item_id, body }).await,
        None => invalid_json(),
    }
}
async fn delete(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
) -> Response {
    dispatch(state, headers, InventoryAction::Delete { item_id }).await
}

//! Inventory component routes; serving composition remains separate.
use crate::{
    AppState, InventoryAction, InventoryInput,
    auth::{VerificationSettings, verify_authorization},
    execute_inventory,
};
use axum::{
    Router,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
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
fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({"detail":{"code":code,"message":message}})),
    )
        .into_response()
}
async fn dispatch(state: RouteState, headers: HeaderMap, action: InventoryAction) -> Response {
    if headers.contains_key("x-verdaxis-market-support-context") {
        return error(
            StatusCode::FORBIDDEN,
            "SUPPORT_CONTEXT_INVALID",
            "Support context is not valid for inventory",
        );
    }
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
    let deleting = matches!(action, InventoryAction::Delete(_));
    match execute_inventory(state.app, access, action).await {
        Ok(_) if deleting => StatusCode::NO_CONTENT.into_response(),
        Ok(body) => axum::Json(body).into_response(),
        Err(failure) => failure.into_response(),
    }
}
fn page(raw: Option<String>) -> Result<(i64, i64), &'static str> {
    let mut skip = 0;
    let mut limit = 50;
    for (key, value) in url::form_urlencoded::parse(raw.as_deref().unwrap_or_default().as_bytes()) {
        if key == "skip" || key == "limit" {
            let parsed = value
                .parse::<i64>()
                .map_err(|_| "Pagination is not valid")?;
            if (key == "skip" && parsed < 0) || (key == "limit" && !(1..=100).contains(&parsed)) {
                return Err("Pagination is outside its range");
            }
            if key == "skip" {
                skip = parsed
            } else {
                limit = parsed
            }
        }
    }
    Ok((skip, limit))
}
async fn list(
    State(state): State<RouteState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    match page(raw) {
        Ok((skip, limit)) => dispatch(state, headers, InventoryAction::List { skip, limit }).await,
        Err(message) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            message,
        ),
    }
}
async fn listings(
    State(state): State<RouteState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    match page(raw) {
        Ok((skip, limit)) => {
            dispatch(state, headers, InventoryAction::Listings { skip, limit }).await
        }
        Err(message) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            message,
        ),
    }
}
async fn create(
    State(state): State<RouteState>,
    headers: HeaderMap,
    body: Result<axum::Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body.0,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
                "A JSON inventory record is required",
            );
        }
    };
    match InventoryInput::create(body, &state.app.policy) {
        Ok(input) => dispatch(state, headers, InventoryAction::Create(input)).await,
        Err(error) => error.into_response(),
    }
}
async fn update(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<axum::Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let id = match id.parse() {
        Ok(id) => id,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
                "Inventory identifier is not valid",
            );
        }
    };
    let body = match body {
        Ok(body) => body.0,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
                "A JSON inventory patch is required",
            );
        }
    };
    match InventoryInput::patch(body) {
        Ok(input) => dispatch(state, headers, InventoryAction::Patch { id, input }).await,
        Err(error) => error.into_response(),
    }
}
async fn delete(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    match id.parse() {
        Ok(id) => dispatch(state, headers, InventoryAction::Delete(id)).await,
        Err(_) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            "Inventory identifier is not valid",
        ),
    }
}

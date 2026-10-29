//! Trade lifecycle component routes; serving activation is separate.
use crate::{
    AppState,
    auth::VerificationSettings,
    market_lifecycle::{
        self, AuditContext, LifecycleAction, LifecycleRequest, MarketErrorCode, MarketWriteError,
    },
};
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{post, put},
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    #[cfg(feature = "contract-tests")]
    collision: Option<Arc<ReceiptCollision>>,
}
pub fn router(app: AppState, verification: VerificationSettings) -> Router {
    Router::new()
        .route("/api/trades/{trade_id}/confirm", put(confirm))
        .route("/api/trades/{trade_id}/decline", put(decline))
        .route("/api/trades/{trade_id}/deliver", put(deliver))
        .route("/api/trades/{trade_id}/pay", post(pay))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            #[cfg(feature = "contract-tests")]
            collision: None,
        })
}
async fn confirm(State(state): State<RouteState>, request: Request) -> Response {
    handle(state, request, LifecycleAction::Confirm).await
}
async fn decline(State(state): State<RouteState>, request: Request) -> Response {
    handle(state, request, LifecycleAction::Decline).await
}
fn validation(status: StatusCode) -> MarketWriteError {
    let mut e = MarketWriteError::new(MarketErrorCode::ValidationFailed);
    e.status = status;
    e
}
async fn handle(state: RouteState, request: Request, action: LifecycleAction) -> Response {
    let (parts, _) = request.into_parts();
    let header = |name| parts.headers.get(name).and_then(|v| v.to_str().ok());
    if parts
        .headers
        .get("x-verdaxis-market-support-context")
        .is_some_and(|v| {
            !v.as_bytes().is_empty() && !v.as_bytes().iter().all(u8::is_ascii_whitespace)
        })
    {
        return (StatusCode::FORBIDDEN,axum::Json(json!({"detail":{"code":"MARKET_SUPPORT_MUTATION_NOT_ALLOWED","message":"This operation is not available in Market Support mode"}}))).into_response();
    }
    let access = match crate::auth::verify_authorization(
        header("authorization"),
        &state.verification,
        Utc::now(),
    ) {
        Ok(access) => access,
        Err(_) => {
            return MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid).into_response();
        }
    };
    let result = async {
        market_lifecycle::authenticate_request(&state.app, &access, action).await?;
        let id = parts
            .uri
            .path()
            .trim_end_matches('/')
            .rsplit('/')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| validation(StatusCode::UNPROCESSABLE_ENTITY))?;
        let key = parts
            .headers
            .get("idempotency-key")
            .map(|value| {
                let text = value
                    .to_str()
                    .map_err(|_| validation(StatusCode::BAD_REQUEST))?
                    .trim();
                if text.is_empty() || text.chars().count() > 255 {
                    return Err(validation(StatusCode::BAD_REQUEST));
                }
                Ok(text.to_owned())
            })
            .transpose()?;
        let (ip, request_id) = crate::runtime::http::audit_identity(&parts);
        let result = market_lifecycle::dispatch(
            state.app,
            access,
            LifecycleRequest {
                id,
                action,
                key,
                audit: AuditContext::new(Some(ip), Some(request_id))?,
                #[cfg(feature = "contract-tests")]
                collision: state.collision,
            },
        )
        .await?;
        Ok((
            StatusCode::from_u16(result.response_status)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            axum::Json(result.response_body),
        )
            .into_response())
    }
    .await;
    result.unwrap_or_else(|error: MarketWriteError| error.into_response())
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str, kyc_gate: bool) -> Router {
    contract_router_inner(pool, key, kyc_gate, None)
}
#[cfg(feature = "contract-tests")]
pub use crate::market_lifecycle::ReceiptCollision;
#[cfg(feature = "contract-tests")]
pub fn contract_router_with_collision(
    pool: sqlx::PgPool,
    key: &str,
    hook: Arc<ReceiptCollision>,
) -> Router {
    contract_router_inner(pool, key, true, Some(hook))
}
#[cfg(feature = "contract-tests")]
fn contract_router_inner(
    pool: sqlx::PgPool,
    key: &str,
    kyc_gate: bool,
    collision: Option<Arc<ReceiptCollision>>,
) -> Router {
    let config=serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_trade_freeze","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":kyc_gate})).unwrap();
    let app = AppState {
        config: Arc::new(config),
        pools: crate::runtime::db::Pools {
            read: pool.clone(),
            health: pool,
        },
        policy: Arc::new(
            serde_json::from_str(include_str!(
                "../../../../../deploy/rust/market-policy.json"
            ))
            .unwrap(),
        ),
        reads: Arc::new(tokio::sync::Semaphore::new(8)),
        reference_cache: Arc::new(Default::default()),
    };
    Router::new()
        .route("/api/trades/{trade_id}/confirm", put(confirm))
        .route("/api/trades/{trade_id}/decline", put(decline))
        .route("/api/trades/{trade_id}/deliver", put(deliver))
        .route("/api/trades/{trade_id}/pay", post(pay))
        .with_state(RouteState {
            app,
            verification: Arc::new(VerificationSettings::new(
                key.into(),
                None,
                "lifecycle-tests".into(),
                "lifecycle-tests".into(),
            )),
            collision,
        })
}

async fn deliver(State(state): State<RouteState>, request: Request) -> Response {
    handle_settlement(state, request, false).await
}
async fn pay(State(state): State<RouteState>, request: Request) -> Response {
    handle_settlement(state, request, true).await
}
async fn handle_settlement(state: RouteState, request: Request, pay: bool) -> Response {
    let (parts, body) = request.into_parts();
    if parts
        .headers
        .get("x-verdaxis-market-support-context")
        .is_some_and(|v| {
            !v.as_bytes().is_empty() && !v.as_bytes().iter().all(u8::is_ascii_whitespace)
        })
    {
        return (StatusCode::FORBIDDEN,axum::Json(json!({"detail":{"code":"MARKET_SUPPORT_MUTATION_NOT_ALLOWED","message":"This operation is not available in Market Support mode"}}))).into_response();
    }
    let access = match crate::auth::verify_authorization(
        parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        &state.verification,
        Utc::now(),
    ) {
        Ok(access) => access,
        Err(_) => {
            return MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid).into_response();
        }
    };
    let result = async {
        market_lifecycle::authenticate_settlement(&state.app, &access, pay).await?;
        let delivery = if pay {
            None
        } else {
            let bytes = axum::body::to_bytes(body, 64 * 1024)
                .await
                .map_err(|_| validation(StatusCode::PAYLOAD_TOO_LARGE))?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| validation(StatusCode::UNPROCESSABLE_ENTITY))?;
            Some(market_lifecycle::Delivery::parse(&value)?)
        };
        let id = parts
            .uri
            .path()
            .trim_end_matches('/')
            .rsplit('/')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| validation(StatusCode::UNPROCESSABLE_ENTITY))?;
        let key = parts
            .headers
            .get("idempotency-key")
            .map(|v| {
                let text = v
                    .to_str()
                    .map_err(|_| validation(StatusCode::BAD_REQUEST))?
                    .trim();
                if text.is_empty() || text.chars().count() > 255 {
                    return Err(validation(StatusCode::BAD_REQUEST));
                }
                Ok(text.to_owned())
            })
            .transpose()?;
        let (ip, request_id) = crate::runtime::http::audit_identity(&parts);
        let result = market_lifecycle::dispatch_settlement(
            state.app,
            access,
            market_lifecycle::SettlementRequest {
                id,
                delivery,
                key,
                audit: AuditContext::new(Some(ip), Some(request_id))?,
                #[cfg(feature = "contract-tests")]
                collision: state.collision,
            },
        )
        .await?;
        Ok((
            StatusCode::from_u16(result.response_status)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            axum::Json(result.response_body),
        )
            .into_response())
    }
    .await;
    result.unwrap_or_else(|error: MarketWriteError| error.into_response())
}

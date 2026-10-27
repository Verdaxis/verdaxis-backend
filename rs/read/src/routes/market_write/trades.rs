//! Direct-take and private-trade component routes; not activated for serving.
use crate::{
    AppState,
    analytics::{
        events::{
            AllowedProperty, AllowedPropertyValue, AllowedServerEvent, DomainCatalog,
            RegisteredEvent, ServerEvent,
        },
        sender::{AnalyticsSender, RequestMetadata},
    },
    auth::VerificationSettings,
    market_trades::{
        self as trades, AuditContext, MarketErrorCode, MarketWriteError, TradeCreate, TradePage,
        TradeRequest,
    },
};
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    support_enabled: bool,
    fees: crate::subscriptions::FeeConfig,
    analytics: Option<AnalyticsSender>,
}
pub fn router(
    app: AppState,
    verification: VerificationSettings,
    support_enabled: bool,
    fees: crate::subscriptions::FeeConfig,
    analytics: Option<AnalyticsSender>,
) -> Router {
    Router::new()
        .route("/api/trades/", post(create))
        .route("/api/trades", post(create))
        .route("/api/trades/my", get(list))
        .route("/api/trades/summary", get(summary))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            support_enabled,
            fees,
            analytics,
        })
}
async fn create(State(state): State<RouteState>, request: Request) -> Response {
    handle(state, request, true, false).await
}
async fn list(State(state): State<RouteState>, request: Request) -> Response {
    handle(state, request, false, false).await
}
async fn summary(State(state): State<RouteState>, request: Request) -> Response {
    handle(state, request, false, true).await
}
fn bad(status: StatusCode) -> MarketWriteError {
    let mut e = MarketWriteError::new(MarketErrorCode::ValidationFailed);
    e.status = status;
    e
}
async fn handle(state: RouteState, request: Request, create: bool, summary: bool) -> Response {
    let (parts, body) = request.into_parts();
    let now = Utc::now();
    let header = |name| parts.headers.get(name).and_then(|v| v.to_str().ok());
    if create && header("x-verdaxis-market-support-context").is_some_and(|v| !v.trim().is_empty()) {
        return (StatusCode::FORBIDDEN,axum::Json(json!({"detail":{"code":"MARKET_SUPPORT_MUTATION_NOT_ALLOWED","message":"This operation is not available in Market Support mode"}}))).into_response();
    }
    let access = match crate::auth::verify_authorization(
        header("authorization"),
        &state.verification,
        now,
    ) {
        Ok(a) => a,
        Err(_) => {
            return MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid).into_response();
        }
    };
    let result = async {
        // Only the reads carry a page; a take never inspects the query string.
        let page = if create {
            TradePage::default()
        } else {
            TradePage::parse(parts.uri.query())?
        };
        let context = header("x-verdaxis-market-support-context")
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse().map_err(|_| {
                    let mut e = MarketWriteError::new(MarketErrorCode::MarketSupportContextInvalid);
                    e.status = StatusCode::BAD_REQUEST;
                    e
                })
            })
            .transpose()?;
        let input = if create {
            let bytes = axum::body::to_bytes(body, 64 * 1024)
                .await
                .map_err(|_| bad(StatusCode::PAYLOAD_TOO_LARGE))?;
            Some(TradeCreate::parse(
                serde_json::from_slice::<Value>(&bytes)
                    .map_err(|_| bad(StatusCode::UNPROCESSABLE_ENTITY))?,
            )?)
        } else {
            None
        };
        // Only a take reads the key: a read never parses or validates it.
        let key = if create {
            header("idempotency-key")
                .filter(|v| !v.is_empty())
                .map(|v| {
                    let key = v.trim();
                    if key.is_empty() || key.chars().count() > 255 {
                        return Err(bad(StatusCode::BAD_REQUEST));
                    }
                    Ok(key.to_owned())
                })
                .transpose()?
        } else {
            None
        };
        let (ip, request_id) = crate::runtime::http::audit_identity(&parts);
        let audit = AuditContext::new(Some(ip), Some(request_id))?;
        let result = trades::dispatch(
            state.app.clone(),
            access,
            TradeRequest {
                create: input,
                key,
                context,
                page,
                summary,
                audit,
                support_enabled: state.support_enabled,
                now,
            },
            state.fees,
        )
        .await?;
        if create
            && !result.replayed
            && result.response_status == 200
            && let Some(sender) = &state.analytics
        {
            let event = RegisteredEvent::Server(AllowedServerEvent::TradeCreated);
            let catalog = DomainCatalog::from_policy(&state.app.policy, Vec::<String>::new());
            let side = if result.response_body["initiated_by"] == "BUYER" {
                "BID"
            } else {
                "ASK"
            };
            let role = if side == "BID" { "BUYER" } else { "SUPPLIER" };
            let mut properties = Vec::new();
            for (key, value) in [
                (AllowedProperty::Role, Some(role)),
                (AllowedProperty::Side, Some(side)),
                (
                    AllowedProperty::CanonicalProduct,
                    result.response_body["market_product"].as_str(),
                ),
                (
                    AllowedProperty::DeliveryPoint,
                    result.response_body["delivery_point_name"].as_str(),
                ),
                (
                    AllowedProperty::AvailabilityWindow,
                    result.response_body["availability_window"].as_str(),
                ),
            ] {
                if let Some(value) =
                    value.and_then(|v| AllowedPropertyValue::parse(event, key, v, &catalog))
                {
                    properties.push((key, value));
                }
            }
            if let Ok(event) = ServerEvent::new(AllowedServerEvent::TradeCreated, properties) {
                sender.schedule(
                    event,
                    RequestMetadata::new(
                        header("user-agent"),
                        parts.uri.host().or_else(|| header("host")),
                    ),
                    header("x-monitor-token"),
                );
            }
        }
        let status = StatusCode::from_u16(result.response_status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Ok((status, axum::Json(result.response_body)).into_response())
    }
    .await;
    result.unwrap_or_else(|e: MarketWriteError| e.into_response())
}
#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str, support_enabled: bool) -> Router {
    contract_router_with_sender(pool, key, support_enabled, None)
}

/// The same component router with a conversion sender attached, for the
/// post-commit delivery cases; serving composition stays integration-owned.
#[cfg(feature = "contract-tests")]
pub fn contract_router_with_sender(
    pool: sqlx::PgPool,
    key: &str,
    support_enabled: bool,
    analytics: Option<AnalyticsSender>,
) -> Router {
    let config=serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_trade_freeze","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100})).unwrap();
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
    router(
        app,
        VerificationSettings::new(
            key.into(),
            None,
            "trades-tests".into(),
            "trades-tests".into(),
        ),
        support_enabled,
        Default::default(),
        analytics,
    )
}

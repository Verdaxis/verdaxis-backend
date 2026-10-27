//! Component routes for cancellation and private owner order reads.
use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{
        ConnectInfo, Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::market_orders::{
    AuditContext, MarketErrorCode, MarketWriteError, OrderRequest, dispatch, refusal,
};
use crate::{AppState, auth::VerificationSettings};

/// Request input the client must correct before it can be accepted.
fn invalid_request(status: StatusCode, message: &'static str) -> MarketWriteError {
    refusal(MarketErrorCode::ValidationFailed, status, message)
}

#[derive(Clone, Debug, Default)]
pub struct CancelInput {
    pub reason: Option<String>,
    pub if_match: Option<String>,
    pub key: Option<String>,
    pub post: bool,
}
impl CancelInput {
    pub fn parse(
        post: bool,
        body: Option<Value>,
        etag: Option<&str>,
        key: Option<&str>,
    ) -> Result<Self, MarketWriteError> {
        let reason = if let Some(body) = body.filter(|body| !body.is_null()) {
            let reason = body.get("reason").and_then(Value::as_str).ok_or_else(|| {
                invalid_request(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "A cancellation reason is required",
                )
            })?;
            if !(3..=500).contains(&reason.chars().count()) || reason.trim().is_empty() {
                return Err(invalid_request(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "A cancellation reason must have 3-500 characters",
                ));
            }
            Some(reason.trim().to_owned())
        } else {
            None
        };
        let key = key
            .map(|key| {
                let key = key.trim();
                if key.is_empty() || key.chars().count() > 255 {
                    Err(invalid_request(
                        StatusCode::BAD_REQUEST,
                        "Idempotency-Key must have 1-255 characters",
                    ))
                } else {
                    Ok(key.to_owned())
                }
            })
            .transpose()?;
        Ok(Self {
            reason,
            if_match: etag.map(str::to_owned),
            key,
            post,
        })
    }
    pub fn check_etag(&self, id: Uuid, version: i32) -> Result<(), MarketWriteError> {
        let raw = self
            .if_match
            .as_deref()
            .ok_or_else(|| MarketWriteError::new(MarketErrorCode::IfMatchRequired))?
            .trim();
        if raw.is_empty() || !raw.starts_with("W/\"") || !raw.ends_with('"') {
            return Err(MarketWriteError::new(MarketErrorCode::IfMatchMalformed));
        }
        if raw != order_etag(id, version) {
            return Err(MarketWriteError::new(MarketErrorCode::OrderVersionStale));
        }
        Ok(())
    }
}
pub fn order_etag(id: Uuid, version: i32) -> String {
    format!("W/\"order-{id}-v{version}\"")
}
#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    support_enabled: bool,
}
/// Add order entry without replacing cancellation or owner-read routes.
/// This component composition does not activate any serving route.
pub fn router_with_create(
    app: AppState,
    order_verification: VerificationSettings,
    create_verification: VerificationSettings,
    settings: super::order_create::CreateSettings,
) -> Router {
    router_with_support(app.clone(), order_verification, settings.support_enabled).merge(
        super::order_create::router(app, create_verification, settings),
    )
}

pub fn router(app: AppState, verification: VerificationSettings) -> Router {
    router_with_support(app, verification, false)
}

pub fn router_with_support(
    app: AppState,
    verification: VerificationSettings,
    support_enabled: bool,
) -> Router {
    Router::new()
        .route("/api/orderbook/my", get(owner_orders))
        .route(
            "/api/orderbook/my/latest-ask-template",
            get(latest_template),
        )
        .route("/api/orderbook/{id}", delete(delete_order))
        .route("/api/orderbook/{id}/cancel", post(post_cancel))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            support_enabled,
        })
}
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    skip: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}
fn default_limit() -> i64 {
    50
}
async fn delete_order(
    State(state): State<RouteState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
    body: Bytes,
) -> Response {
    let id = match path {
        Ok(Path(id)) => id,
        Err(_) => {
            return invalid_request(StatusCode::UNPROCESSABLE_ENTITY, "Order ID must be a UUID")
                .into_response();
        }
    };
    handle(
        state,
        headers,
        Some(id),
        false,
        body,
        Page { skip: 0, limit: 50 },
        false,
        peer.map(|p| p.0.0.ip()),
    )
    .await
}
async fn post_cancel(
    State(state): State<RouteState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
    body: Bytes,
) -> Response {
    let id = match path {
        Ok(Path(id)) => id,
        Err(_) => {
            return invalid_request(StatusCode::UNPROCESSABLE_ENTITY, "Order ID must be a UUID")
                .into_response();
        }
    };
    handle(
        state,
        headers,
        Some(id),
        true,
        body,
        Page { skip: 0, limit: 50 },
        false,
        peer.map(|p| p.0.0.ip()),
    )
    .await
}
async fn owner_orders(
    State(state): State<RouteState>,
    headers: HeaderMap,
    page: Result<Query<Page>, QueryRejection>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
) -> Response {
    // Authentication precedes query validation in the handler.
    let page = page
        .map(|Query(page)| page)
        .unwrap_or(Page { skip: -1, limit: 0 });
    handle(
        state,
        headers,
        None,
        false,
        Bytes::new(),
        page,
        false,
        peer.map(|p| p.0.0.ip()),
    )
    .await
}
async fn latest_template(
    State(state): State<RouteState>,
    headers: HeaderMap,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
) -> Response {
    handle(
        state,
        headers,
        None,
        false,
        Bytes::new(),
        Page { skip: 0, limit: 1 },
        true,
        peer.map(|p| p.0.0.ip()),
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn handle(
    state: RouteState,
    headers: HeaderMap,
    id: Option<Uuid>,
    post: bool,
    body: Bytes,
    page: Page,
    template: bool,
    peer: Option<std::net::IpAddr>,
) -> Response {
    let now = chrono::Utc::now();
    let access = match crate::auth::verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &state.verification,
        now,
    ) {
        Ok(access) => access,
        Err(_) => {
            return MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid).into_response();
        }
    };
    let result = async {
        if body.len() > 64 * 1024 {
            return Err(invalid_request(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The request body is too large",
            ));
        }
        if page.skip < 0 || !(1..=100).contains(&page.limit) {
            return Err(invalid_request(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Pagination is outside the permitted range",
            ));
        }
        let body = if body.is_empty() {
            None
        } else {
            Some(serde_json::from_slice(&body).map_err(|_| {
                invalid_request(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "The JSON body is not valid",
                )
            })?)
        };
        let header = |name: &str| -> Result<Option<&str>, MarketWriteError> {
            headers
                .get(name)
                .map(|v| {
                    v.to_str().map_err(|_| {
                        invalid_request(StatusCode::BAD_REQUEST, "A request header is not valid")
                    })
                })
                .transpose()
        };
        let context_error = |status| {
            refusal(
                MarketErrorCode::MarketSupportContextInvalid,
                status,
                "The support context is not valid",
            )
        };
        let context = header("X-Verdaxis-Market-Support-Context")?
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse()
                    .map_err(|_| context_error(StatusCode::BAD_REQUEST))
            })
            .transpose()?;
        if context.is_some() && !state.support_enabled {
            return Err(context_error(StatusCode::NOT_FOUND));
        }
        if context.is_some() && id.is_some() && !post {
            return Err(refusal(
                MarketErrorCode::ValidationFailed,
                StatusCode::FORBIDDEN,
                "This operation is not available in Market Support mode",
            ));
        }
        let input = if id.is_some() {
            CancelInput::parse(post, body, header("If-Match")?, header("Idempotency-Key")?)?
        } else {
            CancelInput::default()
        };
        let ip = crate::runtime::http::client_ip(peer, header("x-forwarded-for")?);
        let audit = AuditContext::new(
            Some(ip),
            Some(crate::runtime::logging::request_id(header("x-request-id")?)),
        )?;
        let result = dispatch(
            state.app,
            access,
            OrderRequest {
                id,
                context,
                input,
                skip: page.skip,
                limit: page.limit,
                template,
                audit,
                support_enabled: state.support_enabled,
            },
        )
        .await?;
        if result.response_status == 204 {
            Ok(StatusCode::NO_CONTENT.into_response())
        } else {
            Ok((
                StatusCode::from_u16(result.response_status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                axum::Json(result.response_body),
            )
                .into_response())
        }
    }
    .await;
    result.unwrap_or_else(|error: MarketWriteError| {
        let context_invalid = error.code == MarketErrorCode::MarketSupportContextInvalid;
        let mut response = error.into_response();
        if context_invalid {
            response.headers_mut().insert(
                "x-verdaxis-market-support-context-invalid",
                "true".parse().unwrap(),
            );
            response.headers_mut().insert(
                "x-verdaxis-market-support-context-code",
                "MARKET_SUPPORT_CONTEXT_INVALID".parse().unwrap(),
            );
        }
        response
    })
}

#[cfg(feature = "contract-tests")]
fn contract_parts(pool: sqlx::PgPool, key: &str) -> (AppState, VerificationSettings) {
    let config:crate::Config=serde_json::from_value(serde_json::json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_watchlist_producer","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":6,"pool_max":7,"python_budget":0,"configured_max_connections":100})).unwrap();
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
        reads: Arc::new(tokio::sync::Semaphore::new(6)),
        reference_cache: Arc::new(crate::runtime::cache::ReferenceCache::default()),
    };
    let verification = VerificationSettings::new(
        key.into(),
        None,
        "cancel-tests".into(),
        "cancel-tests".into(),
    );
    (app, verification)
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str) -> Router {
    let (app, verification) = contract_parts(pool, key);
    router_with_support(app, verification, true)
}

/// The component routes inside the complete serving composition: health
/// routes, fallbacks and every serving layer, including the support-context
/// boundary. Nothing is mounted for serving; tests use this router.
#[cfg(feature = "contract-tests")]
pub fn served_contract_router(pool: sqlx::PgPool, key: &str, support_enabled: bool) -> Router {
    let (app, verification) = contract_parts(pool, key);
    let routes = router_with_support(app.clone(), verification, support_enabled);
    crate::runtime::http::router_with(routes.with_state(()), app)
}

//! Component routes for cancellation and private owner order reads.
use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{ConnectInfo, Path, Query, State},
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

#[derive(Clone, Debug, Default)]
pub struct CancelInput {
    pub reason: Option<String>,
    pub if_match: Option<String>,
    pub key: Option<String>,
}
impl CancelInput {
    pub fn parse(
        post: bool,
        body: Option<Value>,
        etag: Option<&str>,
        key: Option<&str>,
    ) -> Result<Self, MarketWriteError> {
        let reason = if let Some(body) = body {
            let reason = body.get("reason").and_then(Value::as_str).ok_or_else(|| {
                refusal(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "A cancellation reason is required",
                )
            })?;
            if !(3..=500).contains(&reason.chars().count()) || reason.trim().is_empty() {
                return Err(refusal(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "A cancellation reason must have 3-500 characters",
                ));
            }
            Some(reason.trim().to_owned())
        } else {
            None
        };
        if post && reason.is_none() {
            return Err(refusal(
                StatusCode::BAD_REQUEST,
                "A cancellation reason is required",
            ));
        }
        if let Some(etag) = etag {
            parse_etag(etag)?;
        }
        let key = key
            .map(|key| {
                let key = key.trim();
                if key.is_empty() || key.chars().count() > 255 {
                    Err(refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
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
        })
    }
    pub fn expected_version(&self, id: Uuid) -> Result<Option<i32>, MarketWriteError> {
        self.if_match
            .as_deref()
            .map(|raw| {
                let (expected, version) = parse_etag(raw)?;
                if expected != id {
                    return Err(refusal(
                        StatusCode::PRECONDITION_FAILED,
                        "Listing changed; reload and retry",
                    ));
                }
                Ok(version)
            })
            .transpose()
    }
}
pub fn order_etag(id: Uuid, version: i32) -> String {
    format!("W/\"order-{id}-v{version}\"")
}
fn parse_etag(raw: &str) -> Result<(Uuid, i32), MarketWriteError> {
    let invalid = || refusal(StatusCode::BAD_REQUEST, "If-Match is malformed");
    let text = raw
        .trim()
        .strip_prefix("W/\"order-")
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(invalid)?;
    let (id, version) = text.rsplit_once("-v").ok_or_else(invalid)?;
    let id = id.parse().map_err(|_| invalid())?;
    let version: i32 = version.parse().map_err(|_| invalid())?;
    if version < 1 {
        return Err(invalid());
    }
    Ok((id, version))
}
#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    support_enabled: bool,
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
    Path(id): Path<Uuid>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
    body: Bytes,
) -> Response {
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
    Path(id): Path<Uuid>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
    body: Bytes,
) -> Response {
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
    Query(page): Query<Page>,
    peer: Option<axum::Extension<ConnectInfo<std::net::SocketAddr>>>,
) -> Response {
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
            return Err(refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The request body is too large",
            ));
        }
        if page.skip < 0 || !(1..=100).contains(&page.limit) {
            return Err(refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Pagination is outside the permitted range",
            ));
        }
        let body = if body.is_empty() {
            None
        } else {
            Some(serde_json::from_slice(&body).map_err(|_| {
                refusal(
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
                        refusal(StatusCode::BAD_REQUEST, "A request header is not valid")
                    })
                })
                .transpose()
        };
        let context = header("X-Verdaxis-Market-Support-Context")?
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.parse().map_err(|_| {
                    refusal(StatusCode::BAD_REQUEST, "The support context is not valid")
                })
            })
            .transpose()?;
        if context.is_some() && !state.support_enabled {
            return Err(refusal(StatusCode::NOT_FOUND, "Support context not found"));
        }
        if context.is_some() && id.is_some() && !post {
            return Err(refusal(
                StatusCode::FORBIDDEN,
                "Use the assisted cancellation route",
            ));
        }
        let input =
            CancelInput::parse(post, body, header("If-Match")?, header("Idempotency-Key")?)?;
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
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str) -> Router {
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
    router_with_support(
        app,
        VerificationSettings::new(
            key.into(),
            None,
            "cancel-tests".into(),
            "cancel-tests".into(),
        ),
        true,
    )
}

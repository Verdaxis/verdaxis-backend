use crate::{ApiError, request_id};
use axum::{
    Router,
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Instant,
};

/// Mirror the deployed Python credentialed-origin boundary, without sharing
/// production browser authority with staging. Test harnesses retain their oracle.
pub fn allowed_browser_origin(environment: &str, origin: &str) -> bool {
    match environment {
        "production" => matches!(
            origin,
            "https://app.verdaxis.exchange"
                | "https://verdaxis.exchange"
                | "https://canary.verdaxis.exchange"
        ),
        _ => origin == "https://staging.verdaxis.exchange",
    }
}

pub fn client_ip(peer: Option<IpAddr>, forwarded: Option<&str>) -> String {
    if peer.is_some_and(|ip| ip.is_loopback())
        && let Some(ip) = forwarded
            .and_then(|value| value.rsplit(',').next())
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
    {
        return ip.to_string();
    }
    peer.map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[derive(Clone)]
struct HttpState {
    started: Instant,
    buckets: Arc<Mutex<HashMap<(String, String), RateBucket>>>,
    phase2_limits: Arc<Mutex<crate::auth::limits::Phase2Limits>>,
}

pub fn middleware(routes: Router) -> Router {
    compose(routes, None)
}

pub fn middleware_with_state(routes: Router, app_state: crate::AppState) -> Router {
    compose(routes, Some(app_state))
}

fn compose(routes: Router, app_state: Option<crate::AppState>) -> Router {
    let state = HttpState {
        started: Instant::now(),
        buckets: Arc::new(Mutex::new(HashMap::new())),
        phase2_limits: Arc::new(Mutex::new(crate::auth::limits::Phase2Limits::default())),
    };
    let environment = app_state
        .as_ref()
        .map(|state| state.config.environment.clone())
        .unwrap_or_else(|| "staging".into());
    let routes = routes.layer(middleware::from_fn_with_state(state, dispatch_boundary));
    let routes = if let Some(app_state) = app_state {
        routes.layer(middleware::from_fn_with_state(
            app_state,
            super::admission::middleware,
        ))
    } else {
        routes
    };
    routes
        .layer(middleware::from_fn_with_state(environment, cors))
        .layer(middleware::from_fn(log_request))
}

/// Marks Python's outer ServerErrorMiddleware response, which bypasses CORS
/// and request-id response decoration. Handled API errors do not use this mark.
#[derive(Clone, Copy)]
pub struct UnhandledServerError;

pub fn unhandled_server_error() -> Response {
    let mut response = (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    response.extensions_mut().insert(UnhandledServerError);
    response
}

async fn log_request(mut request: Request, next: Next) -> Response {
    let id = request_id(
        request
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok()),
    );
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    // Later route and error code reads this correlation id from the request.
    request.extensions_mut().insert(id.clone());
    let started = Instant::now();
    let mut response = next.run(request).await;
    // `error_class` and `sqlstate` stay empty here: only the database failure
    // path knows them, and it reports them through the same event in later
    // route tasks. Never add SQL parameters, URLs, headers, or bodies.
    tracing::info!(
        request_id = id,
        method = method.as_str(),
        route,
        status = response.status().as_u16(),
        duration_ms = started.elapsed().as_secs_f64() * 1000.0,
        error_class = "",
        sqlstate = "",
        "http_request"
    );
    if response
        .extensions()
        .get::<UnhandledServerError>()
        .is_none()
    {
        response
            .headers_mut()
            .insert("x-request-id", id.parse().expect("validated request ID"));
    }
    response
}

async fn cors(State(environment): State<String>, request: Request, next: Next) -> Response {
    let Some(origin) = request.headers().get(header::ORIGIN).cloned() else {
        return next.run(request).await;
    };
    let allowed = origin
        .to_str()
        .is_ok_and(|origin| allowed_browser_origin(&environment, origin));
    let preflight = request.method() == "OPTIONS"
        && request
            .headers()
            .contains_key("access-control-request-method");
    let mut response = if preflight {
        let method = request.headers()["access-control-request-method"]
            .to_str()
            .unwrap_or("");
        let method_allowed =
            ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"].contains(&method);
        let failure = match (allowed, method_allowed) {
            (false, false) => "Disallowed CORS origin, method",
            (false, true) => "Disallowed CORS origin",
            (true, false) => "Disallowed CORS method",
            _ => "OK",
        };
        let mut response = (
            if allowed && method_allowed {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            },
            failure,
        )
            .into_response();
        response.headers_mut().insert(
            "access-control-allow-methods",
            "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT"
                .parse()
                .unwrap(),
        );
        response
            .headers_mut()
            .insert("access-control-max-age", "600".parse().unwrap());
        response
            .headers_mut()
            .insert(header::VARY, "Origin".parse().unwrap());
        if let Some(headers) = request.headers().get("access-control-request-headers") {
            response
                .headers_mut()
                .insert("access-control-allow-headers", headers.clone());
        }
        response
    } else {
        next.run(request).await
    };
    if response
        .extensions()
        .get::<UnhandledServerError>()
        .is_some()
    {
        return response;
    }
    response
        .headers_mut()
        .insert("access-control-allow-credentials", "true".parse().unwrap());
    if !preflight {
        response.headers_mut().insert(
            "access-control-expose-headers",
            "Retry-After, X-Request-ID, X-Verdaxis-Market-Support-Context-Invalid"
                .parse()
                .unwrap(),
        );
    }
    if allowed {
        response
            .headers_mut()
            .insert("access-control-allow-origin", origin);
        let vary = response
            .headers()
            .get(header::VARY)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let vary = if vary.is_empty() || preflight {
            "Origin".to_owned()
        } else {
            format!("{vary}, Origin")
        };
        response
            .headers_mut()
            .insert(header::VARY, vary.parse().unwrap());
    }
    response
}

/// Exact public GET inventory. Private paths and MCP never use slash redirects.
pub const MOUNTED_GET_PATHS: &[&str] = &[
    "/health/live",
    "/health/ready",
    "/health",
    "/api/ports",
    "/api/catalog/products",
    "/api/catalog/delivery-points",
    "/api/news",
    "/api/orderbook",
    "/api/orderbook/snapshot",
    "/api/orderbook/bids",
    "/api/orderbook/asks",
    "/api/orderbook/with-ci",
    "/api/orderbook/aggregated",
    "/api/orderbook/product-counts",
    "/api/orderbook/map-summary",
    "/api/orderbook/map-summary/compact",
    "/api/orderbook/products",
    "/api/orderbook/regions",
    "/api/orderbook/fuel-types",
    "/api/listings",
    "/api/prices",
    "/api/prices/reference",
    "/api/prices/reference/export",
    "/api/trade-tape",
    "/api/curves/forward",
    "/api/curves/forward/export",
    "/api/curves/forward/table",
    "/api/curves/forward/slice",
    "/api/curves/forward/board",
];

fn public_get_path(path: &str) -> bool {
    MOUNTED_GET_PATHS.contains(&path)
        || path
            .strip_prefix("/api/ports/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// Apply the two rules that sit just outside route dispatch: the Market Support
/// context boundary and the fixed-window price limits (AD-01). Admission and
/// CORS are separate outer layers.
async fn dispatch_boundary(
    State(state): State<HttpState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    if !state
        .phase2_limits
        .lock()
        .expect("phase2 rate lock")
        .preauth(&client, path, state.started.elapsed().as_secs())
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "60")],
            axum::Json(serde_json::json!({"detail":"Rate limit exceeded"})),
        )
            .into_response();
    }
    if let Some(response) = crate::auth::scope::known_get_refusal(request.method().as_str(), path) {
        return response;
    }
    // Python's support-scope middleware classifies POST /api/orderbook as an
    // allowed market-support mutation, so that one combination falls through
    // to the route instead of the generic 403 other unclassified mutations get.
    let orderbook_create = request.method() == "POST" && path == "/api/orderbook";
    if !matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS")
        && !orderbook_create
        && request
            .headers()
            .get("x-verdaxis-market-support-context")
            .is_some_and(|value| !value.is_empty())
    {
        // Public reads never become private projections under a support context.
        return ApiError {
            status: StatusCode::FORBIDDEN,
            detail: serde_json::json!({
                "code": "MARKET_SUPPORT_MUTATION_NOT_ALLOWED",
                "message": "This operation is not available in Market Support mode"
            }),
            retry_after: None,
            kind: crate::ApiErrorKind::Expected,
        }
        .into_response();
    }
    let limit = match (request.method().as_str(), path) {
        ("GET", "/api/prices") => 60,
        ("GET", "/api/prices/reference") => 30,
        ("GET", "/api/prices/reference/export") => 10,
        _ => 0,
    };
    if limit != 0 {
        // FastAPI validates typed parameters before SlowAPI invokes the endpoint.
        // Handler-level filter checks still happen after this one shared bucket.
        use crate::routes::prices::query::{Surface, validate};
        let surface = match path {
            "/api/prices" => Surface::Summary,
            "/api/prices/reference" => Surface::Reference,
            _ => Surface::Export,
        };
        if let Err(error) = validate(request.uri().query().unwrap_or_default(), surface) {
            return error.into_response();
        }
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|peer| peer.0.ip());
        let client = client_ip(
            peer,
            request
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok()),
        );
        let now = state.started.elapsed().as_millis() as u64;
        let accepted = {
            let mut buckets = state.buckets.lock().expect("rate bucket lock");
            buckets.retain(|_, bucket| now.saturating_sub(bucket.started_ms) < 60_000);
            admit(
                buckets.entry((client, path.into())).or_insert(RateBucket {
                    started_ms: now,
                    used: 0,
                }),
                limit,
                now,
            )
        };
        if !accepted {
            return ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                detail: serde_json::json!(format!("Rate limit exceeded: {limit} per 1 minute")),
                retry_after: None,
                kind: crate::ApiErrorKind::Expected,
            }
            .into_response();
        }
    }
    // FastAPI's explicit GET operations do not implicitly accept HEAD.
    if request.method() == "HEAD"
        && request.extensions().get::<MatchedPath>().is_some()
        && public_get_path(path)
    {
        let mut response = method_not_allowed().await;
        *response.body_mut() = axum::body::Body::empty();
        return response;
    }
    next.run(request).await
}

pub fn public_routes() -> Router<crate::AppState> {
    Router::new()
        .merge(crate::routes::ports::routes())
        .merge(crate::routes::catalog::routes())
        .merge(crate::routes::news::routes())
        .merge(crate::routes::orderbook_snapshot::routes())
        .merge(crate::routes::orderbook_lists::routes())
        .merge(crate::routes::orderbook_map::routes())
        .merge(crate::routes::listings::routes())
        .merge(crate::routes::prices::routes())
        .merge(crate::routes::trade_tape::routes())
        .merge(crate::routes::curves_legacy::routes())
        .merge(crate::routes::curves_canonical::routes())
        .merge(crate::mcp::mcp_router())
}

pub fn public_router(state: crate::AppState) -> Router {
    router_with(public_routes(), state)
}

pub fn router(state: crate::AppState) -> Router {
    public_router(state)
}

/// Compose the serving router with extra routes merged before the layers.
///
/// The contract test server uses this to serve one route group through exactly
/// the same fallbacks, redirects, and middleware as the serving binary, so it
/// can never mask a routing difference.
pub fn router_with(extra: Router<crate::AppState>, state: crate::AppState) -> Router {
    // The dispatch boundary overrides axum's implicit HEAD handler.
    let routes = Router::new()
        .route("/health/live", axum::routing::get(crate::live))
        .route("/health/ready", axum::routing::get(crate::ready))
        .route("/health", axum::routing::get(crate::ready))
        .merge(extra)
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state.clone());
    middleware_with_state(routes, state)
}

async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "GET")],
        axum::Json(serde_json::json!({"detail":"Method Not Allowed"})),
    )
        .into_response()
}

/// Starlette redirects a trailing slash to the route that matches without it.
async fn fallback(request: Request) -> Response {
    let path = request.uri().path();
    let trimmed = path.trim_end_matches('/');
    if trimmed != path && public_get_path(trimmed) {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost");
        let query = request
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, format!("http://{host}{trimmed}{query}"))],
        )
            .into_response();
    }
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({"detail":"Not Found"})),
    )
        .into_response()
}

pub struct QueryFields(HashMap<String, String>);

#[derive(Debug)]
pub enum QueryValue {
    Integer(i64),
    Uuid(uuid::Uuid),
}

/// Parse Pydantic's decimal integer spelling without losing large-integer bounds.
/// Values outside i64 saturate in i128; all declared bounds are i64.
pub(crate) fn python_integer(text: &str) -> Option<i128> {
    let text = text.trim();
    let (negative, digits) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else {
        (false, text.strip_prefix('+').unwrap_or(text))
    };
    let whole = if let Some((whole, fraction)) = digits.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|b| b == b'0') {
            return None;
        }
        whole
    } else {
        digits
    };
    let mut value = 0_i128;
    for part in whole.split('_') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        for digit in part.bytes() {
            value = value
                .saturating_mul(10)
                .saturating_add(i128::from(digit - b'0'));
        }
    }
    Some(if negative { -value } else { value })
}

pub enum QueryKind {
    Integer { minimum: i64, maximum: i64 },
    Uuid,
}

impl QueryFields {
    pub fn parse(query: &str) -> Self {
        Self(
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
        )
    }

    pub fn validate(
        &self,
        name: &str,
        kind: QueryKind,
        required: bool,
    ) -> Result<Option<QueryValue>, ApiError> {
        let value = self.0.get(name);
        let error = |kind: &str, message: String, context: Option<serde_json::Value>| {
            let mut entry =
                serde_json::json!({"type":kind,"loc":["query",name],"msg":message,"input":value});
            if let Some(context) = context {
                entry["ctx"] = context;
            }
            ApiError {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                detail: serde_json::json!([entry]),
                retry_after: None,
                kind: crate::ApiErrorKind::Expected,
            }
        };
        let Some(value) = value else {
            return if required {
                Err(error("missing", "Field required".into(), None))
            } else {
                Ok(None)
            };
        };
        match kind {
            QueryKind::Integer { minimum, maximum } => {
                let parsed = python_integer(value).ok_or_else(|| {
                    error(
                        "int_parsing",
                        "Input should be a valid integer, unable to parse string as an integer"
                            .into(),
                        None,
                    )
                })?;
                if parsed < i128::from(minimum) {
                    return Err(error(
                        "greater_than_equal",
                        format!("Input should be greater than or equal to {minimum}"),
                        Some(serde_json::json!({"ge":minimum})),
                    ));
                }
                if parsed > i128::from(maximum) {
                    return Err(error(
                        "less_than_equal",
                        format!("Input should be less than or equal to {maximum}"),
                        Some(serde_json::json!({"le":maximum})),
                    ));
                }
                Ok(Some(QueryValue::Integer(parsed as i64)))
            }
            QueryKind::Uuid => {
                let parsed = uuid::Uuid::parse_str(value).map_err(|cause| {
                    let cause = cause.to_string();
                    error(
                        "uuid_parsing",
                        format!("Input should be a valid UUID, {cause}"),
                        Some(serde_json::json!({"error":cause})),
                    )
                })?;
                Ok(Some(QueryValue::Uuid(parsed)))
            }
        }
    }
}

pub struct RateBucket {
    pub started_ms: u64,
    pub used: u32,
}

pub fn admit(bucket: &mut RateBucket, limit: u32, now_ms: u64) -> bool {
    if now_ms.saturating_sub(bucket.started_ms) >= 60_000 {
        bucket.started_ms = now_ms;
        bucket.used = 0;
    }
    if bucket.used >= limit {
        return false;
    }
    bucket.used += 1;
    true
}

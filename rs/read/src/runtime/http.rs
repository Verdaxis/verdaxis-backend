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

/// The one credentialed browser origin in scope for Phase 1.
const STAGING_ORIGIN: &str = "https://staging.verdaxis.exchange";

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
    };
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
        .layer(middleware::from_fn(cors))
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

async fn cors(request: Request, next: Next) -> Response {
    let Some(origin) = request.headers().get(header::ORIGIN).cloned() else {
        return next.run(request).await;
    };
    let allowed = origin == STAGING_ORIGIN;
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

/// The only GET paths this binary mounts. Data routes arrive in later tasks and
/// must be added here with their `router` entry.
const MOUNTED_GET_PATHS: [&str; 3] = ["/health/live", "/health/ready", "/health"];

/// Apply the two rules that sit just outside route dispatch: the Market Support
/// context boundary and the fixed-window price limits (AD-01). Admission and
/// CORS are separate outer layers.
async fn dispatch_boundary(
    State(state): State<HttpState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if !matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS")
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
            }
            .into_response();
        }
    }
    next.run(request).await
}

pub fn router(state: crate::AppState) -> Router {
    router_with(Router::new(), state)
}

/// Compose the serving router with extra routes merged before the layers.
///
/// The contract test server uses this to serve one route group through exactly
/// the same fallbacks, redirects, and middleware as the serving binary, so it
/// can never mask a routing difference.
pub fn router_with(extra: Router<crate::AppState>, state: crate::AppState) -> Router {
    // These use axum's `get`, which also serves HEAD. The recorded Python
    // corpus answers HEAD with 405 and `Allow: GET`, so the exact-routing task
    // must override this; see docs/rust-phase1/progress.md.
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
    // Python joins the allowed-method set, which is unstable for two entries;
    // Task 4 records the exact corpus value and will pin the order if needed.
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "GET, HEAD")],
        axum::Json(serde_json::json!({"detail":"Method Not Allowed"})),
    )
        .into_response()
}

/// Starlette redirects a trailing slash to the route that matches without it.
async fn fallback(request: Request) -> Response {
    let path = request.uri().path();
    let trimmed = path.trim_end_matches('/');
    if trimmed != path && MOUNTED_GET_PATHS.contains(&trimmed) {
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
fn python_integer(text: &str) -> Option<i128> {
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

use crate::auth::{limits::Refusal, origin::trusted_origins};
use crate::{ApiError, request_id};
use axum::{
    Router,
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Instant,
};

/// Use the same closed environment policy as the cookie origin guard.
pub fn allowed_browser_origin(environment: &str, origin: &str) -> bool {
    trusted_origins(environment).contains(&origin)
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

/// The one pre-auth authority path Caddy calls before it chooses an owner.
/// It is loopback-only in deployment and never part of the public inventory.
pub const PREAUTH_PATH: &str = "/internal/admin-preauth";
/// Marks a request the authority already counted. Caddy sets it after a 2xx.
pub const PREAUTH_COUNTED_HEADER: &str = "x-verdaxis-preauth-counted";

#[derive(Clone)]
struct HttpState {
    started: Instant,
    buckets: Arc<Mutex<ClientBuckets>>,
    phase2_limits: crate::auth::limits::SharedLimits,
}

pub fn middleware(routes: Router, environment: &str) -> Router {
    compose(
        routes,
        None,
        crate::auth::limits::SharedLimits::default(),
        environment,
    )
}

pub fn middleware_with_state(routes: Router, app_state: crate::AppState) -> Router {
    compose(
        routes,
        Some(app_state),
        crate::auth::limits::SharedLimits::default(),
        "",
    )
}

/// Compose one pre-auth authority around a route split. Task 22 must attach
/// both real upstream owners here before admin-feedback cutover is permitted.
pub fn middleware_with_limits(
    routes: Router,
    limits: crate::auth::limits::SharedLimits,
    environment: &str,
) -> Router {
    compose(routes, None, limits, environment)
}

fn compose(
    routes: Router,
    app_state: Option<crate::AppState>,
    limits: crate::auth::limits::SharedLimits,
    environment: &str,
) -> Router {
    let state = HttpState {
        started: Instant::now(),
        buckets: Arc::new(Mutex::new(ClientBuckets::default())),
        phase2_limits: limits,
    };
    let environment = app_state
        .as_ref()
        .map(|state| state.config.environment.clone())
        .unwrap_or_else(|| environment.to_owned());
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

/// The pre-auth 429. An auth route gets the auth envelope and the seconds until
/// its window resets; the Phase 2 admin prefix keeps its existing response.
fn preauth_refusal(path: &str, refusal: Refusal) -> Response {
    if path.starts_with("/api/auth/") {
        return refusal.auth_failure().into_response();
    }
    rate_limit_response()
}

fn rate_limit_response() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("retry-after", "60")],
        axum::Json(serde_json::json!({"detail": {
            "code": "RATE_LIMITED",
            "message": "Rate limit exceeded",
        }})),
    )
        .into_response()
}

/// Apply the rules that sit just outside route dispatch: the pre-auth quota
/// authority, the Market Support context boundary and the fixed-window price
/// limits. Admission and CORS are separate outer layers.
async fn dispatch_boundary(
    State(state): State<HttpState>,
    mut request: Request,
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
    // One shared pre-auth authority: Caddy calls this path before it decides
    // whether Python or Rust owns the admin request, so both owners draw from
    // one counter in one process. A refused request never reaches an upstream;
    // the admitted request carries the mark below.
    if request.method() == axum::http::Method::GET && path == PREAUTH_PATH {
        if state
            .phase2_limits
            .admit_admin(&client, chrono::Utc::now().timestamp().max(0) as u64)
        {
            return (StatusCode::NO_CONTENT, [(PREAUTH_COUNTED_HEADER, "1")]).into_response();
        }
        return rate_limit_response();
    }
    // The authority already charged this request; charging it again at its
    // Rust owner would halve the reviewed limit. Caddy deletes any
    // client-supplied mark on the fail-open path.
    let counted = request
        .headers()
        .get(PREAUTH_COUNTED_HEADER)
        .is_some_and(|value| value == "1");
    let quota_now = chrono::Utc::now().timestamp().max(0) as u64;
    if !counted && let Err(refusal) = state.phase2_limits.preauth_check(&client, path, quota_now) {
        return preauth_refusal(path, refusal);
    }
    request
        .extensions_mut()
        .insert(crate::auth::login::PreauthCharged(quota_now));
    let path = request.uri().path();
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
            let mut buckets = match state.buckets.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    // Keep the counters: clearing them would grant fresh quota.
                    // Clear poison while holding the recovered guard so only
                    // this recovery emits a warning, not every later request.
                    let guard = poisoned.into_inner();
                    state.buckets.clear_poison();
                    tracing::warn!(recoveries = 1_u64, "price quota lock recovery");
                    guard
                }
            };
            buckets.accept(&client, path, limit, now)
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

/// Phase 2 settings. Every group draws its endpoint quota from the same
/// authority the ingress gate uses, so one process owns the counters.
///
/// Some groups carry their own state and are already complete; the others
/// still need the application state. Providing that state here makes one
/// complete router, so the composition root can merge it with its own routes.
fn phase2_routes(
    state: &crate::AppState,
    limits: crate::auth::limits::SharedLimits,
) -> Option<Router> {
    let settings = state.phase2.as_ref()?;
    let verification = settings.verification.clone();
    let mut referrals = crate::crud::referrals::ReferralSettings::new(
        verification.clone(),
        settings.frontend_url.clone(),
    );
    referrals.limits = limits.clone();
    referrals.mail = settings.mail.clone();
    referrals.email_from = settings.email_from.clone().into();
    Some(
        Router::new()
            .merge(
                crate::crud::notifications::routes(
                    crate::crud::notifications::NotificationAuthority::new(
                        verification.clone(),
                        limits.clone(),
                    ),
                )
                .with_state(state.clone()),
            )
            .merge(crate::crud::preferences::router(
                state.clone(),
                Arc::new(verification.clone()),
                limits.clone(),
            ))
            .merge(
                crate::crud::feedback::routes(Arc::new(verification.clone()), limits.clone())
                    .with_state(state.clone()),
            )
            .merge(crate::crud::watchlists::router(
                state.clone(),
                crate::crud::watchlists::WatchlistSettings::new(
                    verification.clone(),
                    settings.watchlist_activation,
                ),
            ))
            .merge(crate::crud::alerts::router(
                state.clone(),
                verification,
                limits.clone(),
            ))
            .merge(crate::crud::referrals::routes(referrals).with_state(state.clone())),
    )
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
    let limits = crate::auth::limits::SharedLimits::default();
    // Complete every part with the same application state before merging, so a
    // group that provides its own state and a group that still needs it can
    // meet in one router. The one fallback below replaces any group fallback.
    let routes = Router::new()
        .route("/health/live", axum::routing::get(crate::live))
        .route("/health/ready", axum::routing::get(crate::ready))
        .route("/health", axum::routing::get(crate::ready))
        .with_state(state.clone())
        .merge(extra.with_state(state.clone()));
    let routes = match phase2_routes(&state, limits.clone()) {
        Some(phase2) => routes.merge(phase2),
        None => routes,
    };
    let routes = match super::phase3::routes(&state, limits.clone()) {
        Some(auth) => routes.merge(auth),
        None => routes,
    };
    let routes = routes
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed);
    let environment = state.config.environment.clone();
    compose(routes, Some(state), limits, &environment)
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

    /// The raw text of a query parameter, before any declared conversion.
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
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

/// The address-keyed AD-01 price-limit accounting: one fixed window per
/// client and route, bounded and evicting so a flood of distinct addresses
/// cannot grow memory without limit or make each request O(n).
///
/// Ported from the accepted PD2-SEC-1 pre-auth store: an ordered expiration
/// index keeps expiry and eviction bounded, eviction drops the
/// soonest-expiring entry (the oldest accounting; every entry shares the one
/// 60 s window) and is counted by a warn-level number that names no client,
/// and IPv6 clients are grouped to one /64 bucket with IPv4-mapped addresses
/// folded onto their IPv4 bucket. A refused request never extends its window.
#[derive(Default)]
pub struct ClientBuckets {
    buckets: HashMap<(String, String), (u64, u32)>,
    expiration: BTreeSet<(u64, (String, String))>,
    evictions: u64,
}

/// Every entry shares one window length, so the soonest-expiring entry is the
/// oldest accounting. Refusing the new client instead would let a flood of
/// fresh addresses lock out every legitimate client.
const MAX_BUCKETS: usize = 50_000;

const WINDOW_MS: u64 = 60_000;

// A small fixed batch bounds synchronous cleanup under the shared lock. At
// 32 entries per request, stale accounting drains faster than new clients can
// add it, without making one caller release all 50,000 expired entries.
const EXPIRED_CLEANUP_BUDGET: usize = 32;

impl ClientBuckets {
    /// Reset the caller if expired, clean one bounded batch, then account it.
    pub fn accept(&mut self, client: &str, path: &str, limit: u32, now_ms: u64) -> bool {
        let identity = (network_identity(client), path.into());
        if let Some(&(started_ms, _)) = self.buckets.get(&identity)
            && started_ms.saturating_add(WINDOW_MS) <= now_ms
        {
            self.expiration
                .remove(&(started_ms.saturating_add(WINDOW_MS), identity.clone()));
            self.buckets.remove(&identity);
        }
        for _ in 0..EXPIRED_CLEANUP_BUDGET {
            let Some((expires_at, _)) = self.expiration.first() else {
                break;
            };
            if *expires_at > now_ms {
                break;
            }
            let (_, expired) = self.expiration.pop_first().expect("expiration entry");
            self.buckets.remove(&expired);
        }
        if !self.buckets.contains_key(&identity) {
            if self.buckets.len() >= MAX_BUCKETS {
                // Only the identity being admitted is new; its own entry does
                // not exist yet, so eviction can never drop the caller's own
                // accounting, only the soonest-expiring other entry.
                let (_, evicted) = self.expiration.pop_first().expect("full quota index");
                self.buckets.remove(&evicted);
                self.evictions = self.evictions.saturating_add(1);
                tracing::warn!(evictions = self.evictions, "price quota eviction");
            }
            let expires_at = now_ms.saturating_add(WINDOW_MS);
            self.expiration.insert((expires_at, identity.clone()));
            self.buckets.insert(identity.clone(), (now_ms, 0));
        }
        let bucket = self.buckets.get_mut(&identity).expect("quota entry");
        debug_assert!(bucket.0.saturating_add(WINDOW_MS) > now_ms);
        if bucket.1 >= limit {
            return false;
        }
        bucket.1 += 1;
        true
    }

    /// Test visibility into the bounded size of the accounting.
    pub fn entries(&self) -> usize {
        self.buckets.len()
    }

    /// Evictions since start. A plain number: it names no client.
    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

/// One bucket per IPv4 address; IPv6 grouped to /64, with IPv4-mapped
/// addresses folded onto their IPv4 bucket. Unparseable keys keep their own
/// single shared bucket, so they cannot multiply buckets.
fn network_identity(client: &str) -> String {
    match client.parse::<IpAddr>() {
        Ok(IpAddr::V6(address)) => {
            if let Some(ipv4) = address.to_ipv4_mapped() {
                return ipv4.to_string();
            }
            let prefix = u128::from(address) & (u128::MAX << 64);
            Ipv6Addr::from(prefix).to_string()
        }
        Ok(IpAddr::V4(address)) => address.to_string(),
        Err(_) => "unknown".into(),
    }
}

#[cfg(test)]
mod limiter_tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn poisoned_price_accounting_recovers_without_resetting_spent_quotas() {
        let state = HttpState {
            started: Instant::now(),
            buckets: Arc::new(Mutex::new(ClientBuckets::default())),
            phase2_limits: crate::auth::limits::SharedLimits::default(),
        };
        let paths = [
            ("/api/prices", 60),
            ("/api/prices/reference", 30),
            ("/api/prices/reference/export", 10),
        ];
        {
            let mut buckets = state.buckets.lock().unwrap();
            for (path, limit) in paths {
                for _ in 0..limit {
                    assert!(buckets.accept("unknown", path, limit, 0));
                }
            }
        }
        let buckets = state.buckets.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = buckets.lock().unwrap();
                panic!("test unwind while holding price accounting");
            })
            .join()
            .is_err()
        );
        assert!(state.buckets.is_poisoned());
        let mut routes = Router::new();
        for (path, _) in paths {
            routes = routes.route(path, get(|| async { "prices" }));
        }
        let app = routes.layer(middleware::from_fn_with_state(
            state.clone(),
            dispatch_boundary,
        ));
        for (path, limit) in paths {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(response.headers().get("retry-after").is_none());
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"detail": format!("Rate limit exceeded: {limit} per 1 minute")})
            );
        }
        assert!(!state.buckets.is_poisoned());
        let buckets = state.buckets.lock().unwrap();
        assert_eq!(buckets.entries(), 3);
        assert_eq!(buckets.expiration.len(), 3);
    }

    #[test]
    fn bounded_cleanup_keeps_one_index_tuple_per_identity() {
        let mut store = ClientBuckets::default();
        for index in 0..50_000_u32 {
            assert!(store.accept(
                &std::net::Ipv4Addr::from(index).to_string(),
                "/api/prices",
                1,
                0
            ));
        }
        for now in [60_000, 120_000, 180_000] {
            assert!(store.accept("0.0.195.79", "/api/prices", 1, now));
            assert_eq!(store.expiration.len(), store.buckets.len());
            for (identity, (started, _)) in &store.buckets {
                assert!(
                    store
                        .expiration
                        .contains(&(started + WINDOW_MS, identity.clone()))
                );
            }
        }
    }
}

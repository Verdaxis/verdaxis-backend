use crate::{ApiError, request_id};
use axum::{
    Router,
    body::Body,
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
    request.extensions_mut().insert(id.clone());
    let started = Instant::now();
    let mut response = next.run(request).await;
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
    response
        .headers_mut()
        .insert("x-request-id", id.parse().expect("validated request ID"));
    response
}

async fn cors(request: Request, next: Next) -> Response {
    let Some(origin) = request.headers().get(header::ORIGIN).cloned() else {
        return next.run(request).await;
    };
    let allowed = origin == "https://staging.verdaxis.exchange";
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
        return ApiError { status: StatusCode::FORBIDDEN, detail: serde_json::json!({"code":"MARKET_SUPPORT_MUTATION_NOT_ALLOWED", "message":"This operation is not available in Market Support mode"}), retry_after: None }.into_response();
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
    let head = request.method() == "HEAD";
    let mut response = next.run(request).await;
    if head {
        *response.body_mut() = Body::empty();
    }
    response
}

pub fn router(state: crate::AppState) -> Router {
    use axum::routing::{MethodFilter, on};
    let routes = Router::new()
        .route(
            "/health/live",
            on(MethodFilter::GET, crate::live).head(method_not_allowed),
        )
        .route(
            "/health/ready",
            on(MethodFilter::GET, crate::ready).head(method_not_allowed),
        )
        .route(
            "/health",
            on(MethodFilter::GET, crate::ready).head(method_not_allowed),
        )
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

async fn fallback(request: Request) -> Response {
    let path = request.uri().path();
    let trimmed = path.trim_end_matches('/');
    if trimmed != path && ["/health", "/health/live", "/health/ready"].contains(&trimmed) {
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
    ) -> Result<Option<String>, ApiError> {
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
                let parsed = value.trim().parse::<i64>().map_err(|_| {
                    error(
                        "int_parsing",
                        "Input should be a valid integer, unable to parse string as an integer"
                            .into(),
                        None,
                    )
                })?;
                if parsed < minimum {
                    return Err(error(
                        "greater_than_equal",
                        format!("Input should be greater than or equal to {minimum}"),
                        Some(serde_json::json!({"ge":minimum})),
                    ));
                }
                if parsed > maximum {
                    return Err(error(
                        "less_than_equal",
                        format!("Input should be less than or equal to {maximum}"),
                        Some(serde_json::json!({"le":maximum})),
                    ));
                }
            }
            QueryKind::Uuid => {
                if let Err(cause) = uuid::Uuid::parse_str(value) {
                    let cause = cause.to_string();
                    return Err(error(
                        "uuid_parsing",
                        format!("Input should be a valid UUID, {cause}"),
                        Some(serde_json::json!({"error":cause})),
                    ));
                }
            }
        }
        Ok(Some(value.clone()))
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

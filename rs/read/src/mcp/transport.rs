//! Agent-only transport boundaries shared by every MCP operation.
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

pub fn reject_origin(headers: &HeaderMap) -> Option<Response> {
    headers
        .contains_key(axum::http::header::ORIGIN)
        .then(|| (StatusCode::FORBIDDEN, "Invalid Origin header").into_response())
}

pub fn host_allowed(environment: &str, host: &str) -> bool {
    match environment {
        "staging" => host == "api-staging.verdaxis.exchange",
        "development" | "test" => {
            host == "testserver"
                || host
                    .split_once(':')
                    .is_some_and(|(name, _)| name == "127.0.0.1" || name == "localhost")
        }
        _ => false,
    }
}

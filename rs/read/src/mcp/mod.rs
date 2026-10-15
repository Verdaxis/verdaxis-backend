pub mod decimal;
pub mod prices;
mod server;
pub mod transport;
pub mod voyage;

use crate::AppState;
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::any,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::sync::Arc;
use tower::ServiceExt;

pub fn mcp_router() -> axum::Router<AppState> {
    axum::Router::new().route("/mcp", any(dispatch))
}

async fn dispatch(State(state): State<AppState>, request: Request) -> Response {
    if !matches!(
        state.config.environment.as_str(),
        "development" | "test" | "staging"
    ) {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    if let Some(response) = transport::reject_origin(request.headers()) {
        return response;
    }
    let host = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !transport::host_allowed(&state.config.environment, host) {
        return Response::builder()
            .status(421)
            .body(axum::body::Body::from("Invalid Host header"))
            .expect("fixed Host response");
    }
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 4 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return axum::http::StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let method = serde_json::from_slice::<crate::Json>(&bytes)
        .ok()
        .and_then(|value| value["method"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let request = Request::from_parts(parts, axum::body::Body::from(bytes));
    let service = StreamableHttpService::new(
        move || {
            Ok(server::MarketServer {
                state: state.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts(),
    );
    match service.oneshot(request).await {
        Ok(response) => {
            let (mut parts, body) = response.into_parts();
            let bytes =
                match axum::body::to_bytes(axum::body::Body::new(body), 8 * 1024 * 1024).await {
                    Ok(bytes) => bytes,
                    Err(_) => return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                };
            let body = match serde_json::from_slice::<crate::Json>(&bytes) {
                Ok(value) => transport::python_envelope(value, &method)
                    .to_string()
                    .into_bytes(),
                Err(_) => bytes.to_vec(),
            };
            parts.headers.remove(axum::http::header::CONTENT_LENGTH);
            Response::from_parts(parts, axum::body::Body::from(body))
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

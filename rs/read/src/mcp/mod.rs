pub mod prices;
mod server;
pub mod transport;

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
        return (
            axum::http::StatusCode::MISDIRECTED_REQUEST,
            "Invalid Host header",
        )
            .into_response();
    }
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
        Ok(response) => response.map(axum::body::Body::new),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub mod decimal;
pub mod prices;
mod server;
pub mod transport;
pub mod validation;
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
    // Python permits a standalone stateless stream; rmcp rejects it by default.
    if request.method() == axum::http::Method::GET {
        if !request
            .headers()
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"))
        {
            return transport::rpc_error(
                406,
                crate::Json::Null,
                -32600,
                "Not Acceptable: Client must accept text/event-stream",
            );
        }
        return Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache, no-transform")
            .header("x-accel-buffering", "no")
            .body(axum::body::Body::from_stream(
                futures_util::stream::pending::<Result<String, std::io::Error>>(),
            ))
            .expect("standalone stream response");
    }
    if request.method() == axum::http::Method::DELETE {
        return transport::rpc_error(
            405,
            crate::Json::Null,
            -32600,
            "Method Not Allowed: Session termination not supported",
        );
    }
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 4 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return axum::http::StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let mut input = match serde_json::from_slice::<crate::Json>(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return transport::rpc_error(
                400,
                crate::Json::Null,
                -32700,
                &format!("Parse error: {error}"),
            );
        }
    };
    let method = input["method"].as_str().unwrap_or_default().to_owned();
    if parts
        .headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|version| {
            !["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"].contains(&version)
        })
        && !input["params"]["_meta"].is_object()
    {
        return transport::rpc_error(
            400,
            input["id"].clone(),
            -32602,
            "params._meta must be an object carrying the required 'io.modelcontextprotocol/protocolVersion' and 'io.modelcontextprotocol/clientCapabilities' envelope keys",
        );
    }
    let mut body_bytes = bytes.to_vec();
    if method == "tools/call" {
        let tool = input["params"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let mut arguments = input["params"]
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Err(message) = validation::tool_arguments(&tool, &mut arguments) {
            let result = serde_json::json!({"content":[{"text":format!("Error executing tool {tool}: {message}"),"type":"text"}],"isError":true});
            let envelope = transport::python_envelope(
                serde_json::json!({"id":input["id"],"result":result}),
                &method,
            );
            return Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(envelope.to_string()))
                .expect("validation response");
        }
        input["params"]["arguments"] = arguments;
        body_bytes = input.to_string().into_bytes();
    }
    let mut parts = parts;
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    let request = Request::from_parts(parts, axum::body::Body::from(body_bytes));
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
                Ok(mut value) => {
                    if value["error"]["code"] == -32601 {
                        value["error"] = serde_json::json!({"code":-32601,"message":"Method not found","data":method});
                    }
                    transport::python_envelope(value, &method)
                        .to_string()
                        .into_bytes()
                }
                Err(_) => bytes.to_vec(),
            };
            if parts.status == axum::http::StatusCode::ACCEPTED {
                parts.headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    "application/json".parse().expect("fixed MIME"),
                );
            }
            parts.headers.remove(axum::http::header::CONTENT_LENGTH);
            Response::from_parts(parts, axum::body::Body::from(body))
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

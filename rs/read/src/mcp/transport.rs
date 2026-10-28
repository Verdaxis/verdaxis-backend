//! Agent-only transport boundaries shared by every MCP operation.
use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};

/// Keep protocol field membership and exported tool schemas; key order is not semantic.
pub fn protocol_envelope(mut value: crate::Json, method: &str) -> crate::Json {
    value["jsonrpc"] = serde_json::json!("2.0");
    if method == "tools/list" && value.get("result").is_some() {
        let schemas: crate::Json =
            serde_json::from_str(include_str!("../../../../deploy/rust/mcp-tools.json"))
                .expect("committed tool schemas");
        let registered = value["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        value["result"]["tools"] = crate::Json::Array(
            schemas["tools"]
                .as_array()
                .expect("tool schemas")
                .iter()
                .filter(|schema| registered.iter().any(|tool| tool["name"] == schema["name"]))
                .cloned()
                .collect(),
        );
        if value["result"].get("resultType").is_some() {
            value["result"]["cacheScope"] = serde_json::json!("private");
        }
    }
    value
}

pub fn python_string_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut output = String::from(quote);
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value == quote => {
                output.push('\\');
                output.push(value);
            }
            value if value.is_control() => output.push_str(&format!("\\x{:02x}", value as u32)),
            value => output.push(value),
        }
    }
    output.push(quote);
    output
}

pub fn rpc_error(status: u16, id: crate::Json, code: i32, message: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
                .to_string(),
        ))
        .expect("fixed JSON-RPC response")
}

pub fn reject_origin(headers: &HeaderMap) -> Option<Response> {
    headers.contains_key(axum::http::header::ORIGIN).then(|| {
        Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(axum::body::Body::from("Invalid Origin header"))
            .expect("fixed Origin response")
    })
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

//! Agent-only transport boundaries shared by every MCP operation.
use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};

/// Adapt SDK model field order, not tool payload order or values.
/// Python's MCP models sort envelope fields; structured tool data retains schema order.
pub fn python_envelope(mut value: crate::Json, method: &str) -> crate::Json {
    fn sorted(value: &crate::Json) -> crate::Json {
        match value {
            crate::Json::Object(fields) => {
                let mut keys: Vec<_> = fields.keys().collect();
                keys.sort();
                crate::Json::Object(
                    keys.into_iter()
                        .map(|key| {
                            (
                                key.clone(),
                                if key == "structuredContent"
                                    || key == "inputSchema"
                                    || key == "outputSchema"
                                {
                                    fields[key].clone()
                                } else {
                                    sorted(&fields[key])
                                },
                            )
                        })
                        .collect(),
                )
            }
            crate::Json::Array(values) => crate::Json::Array(values.iter().map(sorted).collect()),
            _ => value.clone(),
        }
    }
    if method == "tools/list" && value.get("result").is_some() {
        let schemas: crate::Json =
            serde_json::from_str(include_str!("../../../../deploy/rust/mcp-tools.json"))
                .expect("committed Python tool schemas");
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
    }
    let mut result = serde_json::json!({"jsonrpc":"2.0","id":value["id"]});
    if let Some(error) = value.get("error") {
        result["error"] = sorted(error);
    }
    if let Some(body) = value.get("result") {
        result["result"] = sorted(body);
    }
    result
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

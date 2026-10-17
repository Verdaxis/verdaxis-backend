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
    fn schema_order(value: &crate::Json) -> crate::Json {
        let fields = value.as_object().expect("tool object schema");
        let mut output = serde_json::Map::new();
        for key in ["properties", "required", "type"] {
            if let Some(value) = fields.get(key) {
                output.insert(key.into(), value.clone());
            }
        }
        for (key, value) in fields {
            if !output.contains_key(key) {
                output.insert(key.clone(), value.clone());
            }
        }
        crate::Json::Object(output)
    }
    fn modern_input(value: &crate::Json) -> crate::Json {
        let mut fields = serde_json::Map::new();
        fields.insert("type".into(), value["type"].clone());
        for (key, value) in value.as_object().expect("input schema") {
            if key != "type" {
                fields.insert(key.clone(), value.clone());
            }
        }
        crate::Json::Object(fields)
    }
    if method == "tools/list" && value.get("result").is_some() {
        let modern = value["result"].get("resultType").is_some();
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
                .map(|schema| serde_json::json!({"annotations":sorted(&schema["annotations"]),"description":schema["description"],"inputSchema":if modern { modern_input(&schema["inputSchema"]) } else { schema_order(&schema["inputSchema"]) },"name":schema["name"],"outputSchema":if modern { schema["outputSchema"].clone() } else { schema_order(&schema["outputSchema"]) }}))
                .collect(),
        );
    }
    let mut result = serde_json::json!({"jsonrpc":"2.0","id":value["id"]});
    if let Some(error) = value.get("error") {
        result["error"] = serde_json::json!({"code":error["code"],"message":error["message"]});
        if let Some(data) = error.get("data") {
            result["error"]["data"] = data.clone();
        }
    }
    if let Some(body) = value.get("result") {
        result["result"] = if method == "tools/list" {
            if body.get("resultType").is_some() {
                let mut ordered = serde_json::Map::new();
                for key in ["cacheScope", "resultType", "tools", "ttlMs"] {
                    if let Some(value) = body.get(key) {
                        ordered.insert(key.into(), value.clone());
                    }
                }
                ordered.insert("cacheScope".into(), serde_json::json!("private"));
                crate::Json::Object(ordered)
            } else {
                body.clone()
            }
        } else {
            sorted(body)
        };
    }
    result
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

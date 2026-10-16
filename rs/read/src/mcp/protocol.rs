//! Validate Python's JSON-RPC envelope before adapting tool arguments.
//! rmcp still owns dispatch; this module only preserves rejection contracts.
use super::validation::diagnostic;
use crate::Json;
use axum::http::HeaderMap;
use serde_json::json;

fn schema_data() -> &'static Json {
    static DATA: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!(
            "../../../../deploy/rust/mcp-protocol-schema.json"
        ))
        .expect("protocol schema")
    });
    &DATA
}
fn check(schema: &Json, value: &Json, path: &str, errors: &mut Vec<String>) {
    if let Some(reference) = schema["$ref"].as_str() {
        return check(
            &schema_data()["schema"]["$defs"][reference.rsplit('/').next().unwrap()],
            value,
            path,
            errors,
        );
    }
    if let Some(expected) = schema.get("const") {
        if value != expected {
            errors.push(diagnostic(
                path,
                &format!("Input should be {}", super::validation::repr(expected)),
                "literal_error",
                value,
            ));
        }
        return;
    }
    if let Some(choices) = schema["anyOf"].as_array() {
        let nullable = choices.iter().any(|choice| choice["type"] == "null");
        if nullable && value.is_null() {
            return;
        }
        if nullable && choices.len() == 2 {
            return check(&choices[0], value, path, errors);
        }
        let mut failures = Vec::new();
        for choice in choices {
            let mut branch = Vec::new();
            let label = match choice["type"].as_str() {
                Some("integer") => "int",
                Some("string") => "str",
                Some("null") => "none",
                _ => "dict[str,any]",
            };
            check(choice, value, &format!("{path}.{label}"), &mut branch);
            if branch.is_empty() {
                return;
            }
            failures.extend(branch);
        }
        errors.extend(failures);
        return;
    }
    match schema["type"].as_str() {
        Some("null") if !value.is_null() => errors.push(diagnostic(
            path,
            "Input should be None",
            "none_required",
            value,
        )),
        Some("string") if !value.is_string() => errors.push(diagnostic(
            path,
            "Input should be a valid string",
            "string_type",
            value,
        )),
        Some("integer")
            if !value.is_i64()
                && !value.is_u64()
                && !value.as_f64().is_some_and(|n| n.fract() == 0.0)
                && value
                    .as_str()
                    .is_none_or(|s| crate::runtime::http::python_integer(s).is_none())
                && !value.is_boolean() =>
        {
            errors.push(diagnostic(
                path,
                "Input should be a valid integer",
                "int_type",
                value,
            ))
        }
        Some("object") => {
            let Some(fields) = value.as_object() else {
                if let Some(title) = schema["title"].as_str() {
                    errors.push(diagnostic(
                        path,
                        &format!("Input should be a valid dictionary or instance of {title}"),
                        "model_type",
                        value,
                    ));
                } else {
                    errors.push(diagnostic(
                        path,
                        "Input should be a valid dictionary",
                        "dict_type",
                        value,
                    ));
                }
                return;
            };
            if let Some(properties) = schema["properties"].as_object() {
                for (key, property) in properties {
                    let location = format!("{path}.{key}");
                    if let Some(value) = fields.get(key) {
                        check(property, value, &location, errors);
                    } else if schema["required"]
                        .as_array()
                        .is_some_and(|required| required.contains(&json!(key)))
                    {
                        errors.push(diagnostic(&location, "Field required", "missing", value));
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn validate(input: &Json, headers: &HeaderMap) -> Result<(), Json> {
    let mut errors = Vec::new();
    let mut valid = false;
    for name in [
        "JSONRPCRequest",
        "JSONRPCNotification",
        "JSONRPCResponse",
        "JSONRPCError",
    ] {
        let mut branch = Vec::new();
        check(
            &schema_data()["schema"]["$defs"][name],
            input,
            name,
            &mut branch,
        );
        if branch.is_empty() {
            valid = true;
            break;
        }
        errors.extend(branch);
    }
    if !valid {
        return Err(
            json!({"jsonrpc":"2.0","id":null,"error":{"code":-32602,"message":format!("Validation error: {} validation errors for union[JSONRPCRequest,JSONRPCNotification,JSONRPCResponse,JSONRPCError]\n{}",errors.len(),errors.join("\n"))}}),
        );
    }
    let version = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok());
    if version.is_none_or(|v| ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"].contains(&v))
    {
        return Ok(());
    }
    let reject = |code, message: String, data: Option<Json>| {
        let mut error = json!({"code":code,"message":message});
        if let Some(data) = data {
            error["data"] = data;
        }
        Err(json!({"jsonrpc":"2.0","id":input["id"],"error":error}))
    };
    let version_key = "io.modelcontextprotocol/protocolVersion";
    let capability_key = "io.modelcontextprotocol/clientCapabilities";
    let Some(meta) = input["params"]["_meta"].as_object() else {
        return reject(
            -32602,
            format!(
                "params._meta must be an object carrying the required '{version_key}' and '{capability_key}' envelope keys"
            ),
            None,
        );
    };
    let missing: Vec<_> = [version_key, capability_key]
        .into_iter()
        .filter(|key| !meta.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return reject(
            -32602,
            format!(
                "params._meta is missing the required envelope key(s): {}",
                missing.join(", ")
            ),
            None,
        );
    }
    if meta[version_key].as_str() != version {
        return reject(
            -32020,
            "mcp-protocol-version header does not match the request envelope's protocol version"
                .into(),
            None,
        );
    }
    if headers.get("mcp-method").and_then(|v| v.to_str().ok()) != input["method"].as_str() {
        return reject(
            -32020,
            "mcp-method header does not match the request body's method".into(),
            None,
        );
    }
    if input["method"] == "tools/call"
        && !input["params"]["name"].is_null()
        && headers.get("mcp-name").and_then(|v| v.to_str().ok()) != input["params"]["name"].as_str()
    {
        return reject(
            -32020,
            "mcp-name header does not match the request body's 'name' parameter".into(),
            None,
        );
    }
    if !schema_data()["modern_versions"]
        .as_array()
        .unwrap()
        .contains(&meta[version_key])
    {
        return reject(
            -32022,
            "Unsupported protocol version".into(),
            Some(
                json!({"supported":schema_data()["modern_versions"],"requested":meta[version_key]}),
            ),
        );
    }
    Ok(())
}

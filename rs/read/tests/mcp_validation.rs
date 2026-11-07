#[path = "support/mcp_expected.rs"]
mod expected;

use verdaxis_read::mcp::validation::tool_arguments;

#[test]
fn input_domain_matches_python_oracle() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../deploy/rust/mcp-validation-vectors.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for vector in vectors.as_array().unwrap() {
        let mut arguments = vector["arguments"].clone();
        let result = tool_arguments(vector["tool"].as_str().unwrap(), &mut arguments);
        let matches = if let Some(error) = vector.get("error") {
            result
                .as_ref()
                .err()
                .and_then(|text| text.strip_prefix("Validation failed: "))
                .map(|text| serde_json::from_str::<serde_json::Value>(text).unwrap())
                == Some(expected::diagnostics(error.as_str().unwrap()))
        } else {
            result.is_ok() && arguments == vector["normalized"]
        };
        if !matches {
            failures.push(format!(
                "{}: {result:?}\nnormalized: {arguments}",
                vector["name"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

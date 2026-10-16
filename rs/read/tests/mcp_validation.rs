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
            result.as_ref().err().map(String::as_str) == error.as_str()
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

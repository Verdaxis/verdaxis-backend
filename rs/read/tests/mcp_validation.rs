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
            use std::io::Write;
            use std::process::{Command, Stdio};
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let mut child = Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
                .current_dir(&root)
                .args(["-m", "scripts.rust_contract.compare_values"])
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let pair = serde_json::json!({"expected": error, "actual": result.as_ref().err(),
                "route_id": format!("P1-tool-{}", vector["tool"].as_str().unwrap()),
                "diagnostic_context": {"tool": vector["tool"], "arguments": vector["arguments"]}});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(pair.to_string().as_bytes())
                .unwrap();
            child.wait().unwrap().success()
        } else {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let mut child = Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
                .current_dir(&root)
                .args(["-m", "scripts.rust_contract.compare_values"])
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let pair = serde_json::json!({"expected": vector["normalized"], "actual": arguments, "validation_tool": vector["tool"]});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(pair.to_string().as_bytes())
                .unwrap();
            result.is_ok() && child.wait().unwrap().success()
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

use verdaxis_read::mcp::voyage::voyage_plan;

#[test]
fn complete_voyage_outputs_match_python() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../deploy/rust/voyage-vectors.json")).unwrap();
    for vector in vectors.as_array().unwrap() {
        let today = vector["today"].as_str().unwrap().parse().unwrap();
        let result = voyage_plan(&vector["request"], &vector["prices"], today);
        if let Some(error) = vector.get("error") {
            assert_eq!(
                result.unwrap_err(),
                error.as_str().unwrap(),
                "{}",
                vector["name"]
            );
        } else {
            assert_eq!(result.unwrap(), vector["result"], "{}", vector["name"]);
        }
    }
}

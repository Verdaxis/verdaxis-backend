use verdaxis_read::mcp::voyage::voyage_plan;

#[test]
fn price_map_keys_are_canonicalized_before_calculation() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../deploy/rust/voyage-vectors.json")).unwrap();
    let vector = &vectors[0];
    let mut request = vector["request"].clone();
    request["assumptions"]["vlsfo_price_usd_per_mt"] = serde_json::json!({
        " shanghai ": "650", "SINGAPORE": "907"
    });
    let result = voyage_plan(
        &request,
        &vector["prices"],
        vector["today"].as_str().unwrap().parse().unwrap(),
    )
    .unwrap();
    assert_eq!(result, vector["result"]);

    request["assumptions"]["vlsfo_price_usd_per_mt"] = serde_json::json!({
        "Shanghai": "1", " shanghai ": "650", "SINGAPORE": "907"
    });
    assert_eq!(
        voyage_plan(
            &request,
            &vector["prices"],
            vector["today"].as_str().unwrap().parse().unwrap(),
        )
        .unwrap(),
        vector["result"]
    );
}

#[test]
fn forbidden_lane_price_is_ignored_by_pure_engine() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../deploy/rust/voyage-vectors.json")).unwrap();
    let vector = &vectors[0];
    let mut request = vector["request"].clone();
    request["legs"] = serde_json::json!([{"from_port":"Rotterdam","to_port":"Singapore"}]);
    let mut price = vector["prices"][0].clone();
    price["lookup"]["port"] = serde_json::json!("Rotterdam");
    price["lookup"]["departure_date"] = request["start_date"].clone();
    price["price"]["port"] = serde_json::json!("Rotterdam");
    let today = vector["today"].as_str().unwrap().parse().unwrap();
    for fuels in [
        serde_json::json!(["VLSFO", "UCOME_B100"]),
        serde_json::json!(["UCOME_B100"]),
    ] {
        request["vessel"]["compatible_fuels"] = fuels;
        let absent = voyage_plan(&request, &serde_json::json!([]), today).unwrap();
        let supplied = voyage_plan(&request, &serde_json::json!([price.clone()]), today).unwrap();
        assert_eq!(supplied, absent);
    }
}

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

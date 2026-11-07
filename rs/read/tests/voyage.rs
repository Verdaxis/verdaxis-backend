use verdaxis_read::mcp::voyage::voyage_plan;

// These are the reviewed economic fields of the frozen voyage contract.
// Decimal spelling may differ; values, JSON kinds, keys and array order may not.
const DECIMAL_PATHS: &[&str] = &[
    "/legs/*/berth_days",
    "/legs/*/berth_energy_mj",
    "/legs/*/berth_eu_scope_share",
    "/legs/*/distance_nm",
    "/legs/*/eu_scope_share",
    "/legs/*/sea_days",
    "/legs/*/sea_energy_mj",
    "/strategies/*/eu_ets_cost_eur",
    "/strategies/*/fuel_cost_usd",
    "/strategies/*/fuel_uses/*/cost_usd",
    "/strategies/*/fuel_uses/*/price/carbon_intensity_gco2e_per_mj",
    "/strategies/*/fuel_uses/*/price/energy_density_mj_per_kg",
    "/strategies/*/fuel_uses/*/price/price_usd_per_mt",
    "/strategies/*/fuel_uses/*/tonnes",
    "/strategies/*/fueleu_balance_tco2e",
    "/strategies/*/fueleu_cost_eur",
    "/strategies/*/fueleu_target_gco2e_per_mj",
    "/strategies/*/ghg_intensity_gco2e_per_mj",
    "/strategies/*/in_scope_energy_mj",
    "/strategies/*/total_cost_usd",
];

fn same_meaning(actual: &serde_json::Value, expected: &serde_json::Value, path: &str) -> bool {
    use serde_json::Value;
    use verdaxis_read::market::amount::Amount;
    match (actual, expected) {
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && right.iter().all(|(key, value)| {
                    left.get(key)
                        .is_some_and(|actual| same_meaning(actual, value, &format!("{path}/{key}")))
                })
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_meaning(left, right, &format!("{path}/*")))
        }
        (Value::String(left), Value::String(right)) if DECIMAL_PATHS.contains(&path) => {
            match (Amount::parse(left), Amount::parse(right)) {
                (Ok(left), Ok(right)) => left.compare(&right).is_eq(),
                _ => false,
            }
        }
        (Value::String(left), Value::String(right))
            if path == "/strategies/*/fuel_uses/*/price/observed_at" =>
        {
            match (
                chrono::DateTime::parse_from_rfc3339(left),
                chrono::DateTime::parse_from_rfc3339(right),
            ) {
                (Ok(left), Ok(right)) => left == right,
                _ => false,
            }
        }
        _ => actual == expected,
    }
}

fn assert_meaning(actual: &serde_json::Value, expected: &serde_json::Value) {
    assert!(
        same_meaning(actual, expected, ""),
        "voyage contract changed"
    );
}

#[test]
fn fixed_comparison_preserves_types_values_keys_and_array_order() {
    use serde_json::json;
    let expected = json!({"legs": [{"distance_nm": "2200", "from_port": "Shanghai"}]});
    assert!(same_meaning(
        &json!({"legs": [{"distance_nm": "22e2", "from_port": "Shanghai"}]}),
        &expected,
        ""
    ));
    for changed in [
        json!({"legs": [{"distance_nm": "2201", "from_port": "Shanghai"}]}),
        json!({"legs": [{"distance_nm": 2200, "from_port": "Shanghai"}]}),
        json!({"legs": [{"distance_nm": "2200"}]}),
        json!({"legs": [{"distance_nm": "2200", "from_port": "Shanghai", "extra": null}]}),
        json!({"legs": [{"distance_nm": "2200", "from_port": "Shanghai"}, {}]}),
    ] {
        assert!(!same_meaning(&changed, &expected, ""));
    }
    assert!(!same_meaning(&json!(["a", "b"]), &json!(["b", "a"]), ""));
    assert!(!same_meaning(
        &json!("22e2"),
        &json!("2200"),
        "/not-an-economic-field"
    ));
}

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
    assert_meaning(&result, &vector["result"]);

    request["assumptions"]["vlsfo_price_usd_per_mt"] = serde_json::json!({
        "Shanghai": "1", " shanghai ": "650", "SINGAPORE": "907"
    });
    assert_meaning(
        &voyage_plan(
            &request,
            &vector["prices"],
            vector["today"].as_str().unwrap().parse().unwrap(),
        )
        .unwrap(),
        &vector["result"],
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
            assert_meaning(&result.unwrap(), &vector["result"]);
        }
    }
}

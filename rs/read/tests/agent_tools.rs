use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use verdaxis_read::{MarketPolicy, agent::tools::*};

fn policy() -> MarketPolicy {
    serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()
}
fn now() -> DateTime<Utc> {
    "2026-11-11T12:00:00Z".parse().unwrap()
}
fn draft() -> Value {
    let p = policy();
    json!({"side":"BID","market_product":p.products[0].code,"delivery_point":p.points[0].name,"availability_window":"SPOT","quantity_mt":"10.00","limit_price_usd":"100.00","expires_at":"2026-11-12T12:00:00Z","source_message_seq":1})
}
fn tool(name: &str, arguments: Value) -> AgentTool {
    serde_json::from_value(json!({"tool":name,"arguments":arguments})).unwrap()
}
fn valid(name: &str, arguments: Value) -> bool {
    tool(name, arguments).validate(&policy(), now()).is_ok()
}
fn digest(arguments: Value) -> String {
    let args: CreateDraftArguments = serde_json::from_value(arguments).unwrap();
    verdaxis_read::market::terms::digest_terms(&args.canonical_terms(&policy(), now()).unwrap())
        .unwrap()
}

#[test]
fn typed_dispatch_refuses_unknown_tools_and_fields() {
    for name in [
        "place_order",
        "amend_order",
        "cancel_order",
        "withdraw_order",
        "get_drafts",
    ] {
        assert!(serde_json::from_value::<AgentTool>(json!({"tool":name,"arguments":{}})).is_err());
    }
    for (name, mut args) in [
        ("list_markets", json!({})),
        (
            "get_market_slice",
            json!({"market_product":"x","delivery_point":"x","availability_window":"SPOT"}),
        ),
        ("plan_voyage_fuel", json!({"request":{}})),
        ("get_my_orders", json!({})),
        ("get_my_trades", json!({})),
        ("create_draft", draft()),
        ("ask_user", json!({"question":"Which port?"})),
    ] {
        args["user_id"] = json!("untrusted");
        assert!(
            serde_json::from_value::<AgentTool>(json!({"tool":name,"arguments":args})).is_err(),
            "{name}"
        );
    }
    assert!(
        serde_json::from_value::<AgentTool>(
            json!({"tool":"list_markets","arguments":{},"extra":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<AgentTool>(
            json!({"tool":"get_my_orders","arguments":{"status":"OTHER"}})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<AgentTool>(
            json!({"tool":"get_my_trades","arguments":{"limit":-1}})
        )
        .is_err()
    );
}
#[test]
fn trade_limit_has_bounded_default_and_endpoints() {
    for value in [json!({}), json!({"limit":1}), json!({"limit":50})] {
        assert!(valid("get_my_trades", value));
    }
    for limit in [0, 51, u32::MAX] {
        assert!(!valid("get_my_trades", json!({"limit":limit})));
    }
}
#[test]
fn draft_decimal_bounds_are_exact() {
    for field in ["quantity_mt", "limit_price_usd"] {
        for value in [
            "0",
            "-1",
            "100000.0000000000000000000001",
            "NaN",
            "Infinity",
            "not decimal",
            "0.00000000000000000000000000001",
        ] {
            let mut args = draft();
            args[field] = json!(value);
            assert!(!valid("create_draft", args), "{field}: {value}");
        }
        for value in ["0.0000000000000000000000000001", "100000"] {
            let mut args = draft();
            args[field] = json!(value);
            assert!(valid("create_draft", args), "{field}: {value}");
        }
    }
    let mut args = draft();
    args["limit_price_usd"] = Value::Null;
    assert!(valid("create_draft", args));
    let mut args = draft();
    args["quantity_mt"] = json!(10);
    assert!(serde_json::from_value::<CreateDraftArguments>(args).is_err());
}
#[test]
fn draft_expiry_and_source_sequence_are_bounded() {
    for expires in [
        now() - Duration::seconds(1),
        now(),
        now() + Duration::days(90) + Duration::microseconds(1),
    ] {
        let mut args = draft();
        args["expires_at"] = json!(expires.to_rfc3339());
        assert!(!valid("create_draft", args));
    }
    for expires in [
        now() + Duration::microseconds(1),
        now() + Duration::days(90),
    ] {
        let mut args = draft();
        args["expires_at"] = json!(expires.to_rfc3339());
        assert!(valid("create_draft", args));
    }
    for expires in [
        json!(null),
        json!("bad timestamp"),
        json!("2026-11-12T12:00:00.000000001Z"),
    ] {
        let mut args = draft();
        args["expires_at"] = expires.clone();
        assert_eq!(
            valid("create_draft", args),
            expires.is_null() || expires == "2026-11-12T12:00:00.000000001Z"
        );
    }
    for seq in [0, -1] {
        let mut args = draft();
        args["source_message_seq"] = json!(seq);
        assert!(!valid("create_draft", args));
    }
    let mut args = draft();
    args["side"] = json!("BUY");
    assert!(serde_json::from_value::<CreateDraftArguments>(args).is_err());
}
#[test]
fn questions_and_options_count_unicode_characters() {
    for length in [1, 500] {
        assert!(valid("ask_user", json!({"question":"é".repeat(length)})));
    }
    for length in [0, 501] {
        assert!(!valid("ask_user", json!({"question":"é".repeat(length)})));
    }
    for length in [1, 80] {
        assert!(valid(
            "ask_user",
            json!({"question":"Port?","options":vec!["é".repeat(length);6]})
        ));
    }
    for length in [0, 81] {
        assert!(!valid(
            "ask_user",
            json!({"question":"Port?","options":["é".repeat(length)]})
        ));
    }
    assert!(!valid(
        "ask_user",
        json!({"question":"Port?","options":vec!["a";7]})
    ));
    assert!(valid("ask_user", json!({"question":"Port?","options":[]})));
}
#[test]
fn market_and_voyage_validation_use_existing_rules() {
    let p = policy();
    let slice = json!({"market_product":p.products[0].code,"delivery_point":p.points[0].name,"availability_window":"SPOT"});
    assert!(valid("get_market_slice", slice.clone()));
    for (field, value) in [
        ("market_product", "unknown"),
        ("delivery_point", "unknown"),
        ("availability_window", "not a window"),
    ] {
        let mut args = slice.clone();
        args[field] = json!(value);
        assert!(!valid("get_market_slice", args));
        let mut args = draft();
        args[field] = json!(value);
        assert!(!valid("create_draft", args));
    }
    assert!(!valid("plan_voyage_fuel", json!({"request":{}})));
    let request = json!({"vessel":{"speed_knots":"14","sea_consumption_mt_per_day":"20","compatible_fuels":["VLSFO"]},"legs":[{"from_port":"Singapore","to_port":"Rotterdam"}],"start_date":"2026-11-11"});
    assert!(valid(
        "plan_voyage_fuel",
        json!({"request":request.clone()})
    ));
    let mut bad = request;
    bad["vessel"]["speed_knots"] = json!("0.5");
    assert!(!valid("plan_voyage_fuel", json!({"request":bad})));
}
#[test]
fn catalog_and_typed_dispatch_have_exact_parity() {
    let args = [
        json!({}),
        json!({"market_product":"x","delivery_point":"x","availability_window":"SPOT"}),
        json!({"request":{}}),
        json!({}),
        json!({}),
        draft(),
        json!({"question":"Port?"}),
    ];
    let catalog = catalog();
    let entries = catalog.as_array().unwrap();
    assert_eq!(entries.len(), 7);
    let schema = serde_json::to_value(rmcp::schemars::schema_for!(AgentTool)).unwrap();
    let dispatch_names: std::collections::BTreeSet<_> = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| {
            variant["properties"]["tool"]["const"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let catalog_names: std::collections::BTreeSet<_> = entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(dispatch_names, catalog_names);
    let names = [
        "list_markets",
        "get_market_slice",
        "plan_voyage_fuel",
        "get_my_orders",
        "get_my_trades",
        "create_draft",
        "ask_user",
    ];
    for ((entry, name), args) in entries.iter().zip(names).zip(args) {
        assert_eq!(entry["name"], name);
        assert_eq!(tool(name, args).name(), name);
        assert_eq!(entry["input_schema"]["additionalProperties"], false);
        assert!(!entry["description"].as_str().unwrap().contains('\n'));
    }
}
#[test]
fn canonical_draft_digest_covers_each_field_and_normalizes_values() {
    let original = draft();
    let expected = digest(original.clone());
    let mut equal = original.clone();
    equal["quantity_mt"] = json!("10");
    equal["limit_price_usd"] = json!("100.0");
    equal["expires_at"] = json!("2026-11-12T20:00:00+08:00");
    assert_eq!(digest(equal), expected);
    let p = policy();
    for (field, value) in [
        ("side", json!("ASK")),
        ("market_product", json!(p.products[1].code)),
        ("delivery_point", json!(p.points[1].name)),
        ("availability_window", json!("2026-12")),
        ("quantity_mt", json!("11")),
        ("limit_price_usd", Value::Null),
        ("expires_at", Value::Null),
        ("source_message_seq", json!(2)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        assert_ne!(digest(changed), expected, "{field}");
    }
}

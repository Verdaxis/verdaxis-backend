use serde_json::{Value, json};
use verdaxis_read::crud::preferences::validate_value;

#[test]
fn preference_http_oracle_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/preferences.json"
    ))
    .unwrap();
    for case in fixture["vectors"].as_array().unwrap() {
        let name = case["id"].as_str().unwrap();
        // Syntax and required-body validation belong to the HTTP adapter.
        if name.starts_with("syntax-") || name == "empty-namespace" || case["value"].is_null() {
            continue;
        }
        let value = case["value"].clone();
        match validate_value(case["namespace"].as_str().unwrap(), value.clone()) {
            Ok(original) => {
                assert_eq!(case["python_status"], 200, "{name}");
                assert_eq!(original, value, "{name}: original payload changed");
            }
            Err(error) => {
                assert_eq!(
                    error.status.as_u16(),
                    case["python_status"].as_u64().unwrap() as u16,
                    "{name}"
                );
                assert_eq!(
                    json!({"detail": error.detail}),
                    case["python_detail"],
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn every_required_field_is_strict_and_present() {
    let fields = [
        "email_trade_updates",
        "email_market_alerts",
        "email_compliance_digest",
        "email_system_announcements",
        "inapp_trade_updates",
        "inapp_market_alerts",
        "inapp_order_matches",
    ];
    let valid: Value = fields
        .iter()
        .map(|key| ((*key).to_owned(), json!(true)))
        .collect();
    for field in fields {
        for invalid in [json!(0), json!(1), json!("true"), json!(null)] {
            let mut body = valid.clone();
            body[field] = invalid;
            assert_eq!(
                validate_value("notifications", body).unwrap_err().status,
                422
            );
        }
        let mut body = valid.clone();
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            validate_value("notifications", body).unwrap_err().status,
            422
        );
    }
    for field in ["products", "portIds"] {
        let mut body = json!({"products": [], "portIds": []});
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            validate_value("market_watch", body).unwrap_err().status,
            422
        );
    }
}

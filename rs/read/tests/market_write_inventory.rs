use serde_json::json;
use verdaxis_read::InventoryInput;

fn policy() -> verdaxis_read::MarketPolicy {
    serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()
}

fn body() -> serde_json::Value {
    json!({"port_id":"fixture-port","fuel_type":"Methanol","current_stock_mt":"0"})
}

#[test]
fn inventory_numbers_keep_their_exact_domains() {
    for value in [json!("0"), json!("100000.00"), json!(0)] {
        let mut input = body();
        input["current_stock_mt"] = value;
        assert!(InventoryInput::create(input, &policy()).is_ok());
    }
    for value in [
        json!(null),
        json!("-0"),
        json!("-1"),
        json!("100000.01"),
        json!("1.001"),
        json!("1e2"),
    ] {
        let mut input = body();
        input["current_stock_mt"] = value;
        assert!(InventoryInput::create(input, &policy()).is_err());
    }
}

#[test]
fn inventory_patch_distinguishes_omission_clear_and_required_values() {
    assert!(InventoryInput::patch(json!({"price_per_mt_usd":null,"product_name":null})).is_ok());
    for field in [
        "current_stock_mt",
        "incoming_stock_mt",
        "certification_declared",
        "msds_available",
        "off_spec",
    ] {
        assert!(
            InventoryInput::patch(json!({field:null})).is_err(),
            "{field}"
        );
    }
    for field in [
        "port_id",
        "fuel_type",
        "is_certified",
        "reserved_stock_mt",
        "owner_user_id",
        "supplier_id",
    ] {
        assert!(
            InventoryInput::patch(json!({field:"other"})).is_err(),
            "{field}"
        );
    }
}

#[test]
fn inventory_declarations_use_the_catalog_profile() {
    let policy = policy();
    for profile in ["fame_v1", "lng_v1"] {
        let product = policy
            .products
            .iter()
            .find(|p| p.qualification_profile == profile)
            .unwrap();
        let mut input = body();
        input["product_name"] = json!(product.name);
        input["fuel_type"] = json!(product.fuel_type);
        assert!(InventoryInput::create(input, &policy).is_err());
    }
    let mut scalar = body();
    scalar["lng_terms"] = json!({"side":"ASK"});
    assert!(InventoryInput::create(scalar, &policy).is_err());
}

#[test]
fn inventory_create_rejects_unclassified_fields() {
    let mut input = body();
    input["reserved_stock_mt"] = json!("1");
    assert!(InventoryInput::create(input, &policy()).is_err());
}

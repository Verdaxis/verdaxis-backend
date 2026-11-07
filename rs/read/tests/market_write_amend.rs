#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use verdaxis_read::routes::market_write::amend::AmendInput;

fn now() -> DateTime<Utc> {
    "2026-10-28T12:00:00Z".parse().unwrap()
}
fn update() -> Value {
    json!({"price_per_mt_usd": "120.00"})
}

#[test]
fn amend_schema_separates_omitted_null_and_supplied_fields() {
    let input = AmendInput::parse(update(), now()).unwrap();
    assert!(input.supplied(&["price_per_mt_usd"]));
    assert!(!input.supplied(&["quantity_mt"]));
    // An explicit null on a clearable field parses and is distinguishable from
    // an omitted field; an economic field's null is refused before any lock.
    let input = AmendInput::parse(json!({"expires_at": null}), now()).unwrap();
    assert!(input.supplied(&["expires_at"]));
    assert!(input.value("expires_at").is_null());
    for field in ["quantity_mt", "price_per_mt_usd", "certifications"] {
        assert_eq!(
            AmendInput::parse(json!({ field: null }), now())
                .unwrap_err()
                .code
                .as_str(),
            "ORDER_FIELD_NULL",
            "{field}"
        );
    }
    // Undeclared fields are ignored entirely.
    let input = AmendInput::parse(
        json!({"price_per_mt_usd": "100", "product_id": "00000000-0000-0000-0000-000000000009"}),
        now(),
    )
    .unwrap();
    assert!(input.value("product_id").is_null());
}

#[test]
fn amend_schema_checks_each_supplied_boundary() {
    for (field, invalid, status) in [
        ("quantity_mt", json!("0"), 422),
        ("quantity_mt", json!("100000.01"), 422),
        ("quantity_mt", json!("1.001"), 422),
        ("quantity_mt", json!("NaN"), 422),
        ("price_per_mt_usd", json!("0"), 422),
        ("price_per_mt_usd", json!("1000000.01"), 422),
        ("price_per_mt_usd", json!("1.001"), 422),
        ("carbon_intensity_gco2_mj", json!("-1"), 422),
        ("expires_at", json!("2026-10-28T12:00:00"), 422),
        ("certifications", json!("cert"), 422),
        ("msds_available", json!("yes"), 422),
    ] {
        let mut value = update();
        value[field] = invalid;
        assert_eq!(
            AmendInput::parse(value, now()).unwrap_err().status.as_u16(),
            status,
            "{field}"
        );
    }
    let mut value = update();
    value["expires_at"] = json!("2026-10-29T12:00:00+08:00");
    assert!(AmendInput::parse(value, now()).is_ok());
}

#[test]
fn amend_hash_tracks_the_normalized_update_payload() {
    let base = AmendInput::parse(json!({"price_per_mt_usd": "120"}), now()).unwrap();
    let same = AmendInput::parse(json!({"price_per_mt_usd": "120.00"}), now()).unwrap();
    assert_eq!(base.payload_hash(), same.payload_hash());
    let changed = AmendInput::parse(json!({"price_per_mt_usd": "121"}), now()).unwrap();
    assert_ne!(base.payload_hash(), changed.payload_hash());
    let omitted = AmendInput::parse(json!({}), now()).unwrap();
    assert_ne!(base.payload_hash(), omitted.payload_hash());
    // An explicit null participates in the hash: clearing is a different
    // command from omitting.
    let cleared = AmendInput::parse(json!({"off_spec_notes": null}), now()).unwrap();
    let not_cleared = AmendInput::parse(json!({}), now()).unwrap();
    assert_ne!(cleared.payload_hash(), not_cleared.payload_hash());
}

use rust_decimal::Decimal as D;
use serde_json::{Value, json};
use std::str::FromStr;
use verdaxis_read::market::live_benchmarks::{candidate_is_benchmark_qualified, qualified_vwap};
use verdaxis_read::market::visibility::VisibilityClock;

fn policy() -> verdaxis_read::MarketPolicy {
    serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()
}

fn clock() -> VisibilityClock {
    VisibilityClock::at("2026-10-15T15:59:59Z".parse().unwrap())
}

fn typed_row(profile: &str) -> Value {
    let vector = match profile {
        "fame_v1" => include_str!("../../../tests/rust_contract/vectors/terms/fame.json"),
        _ => include_str!("../../../tests/rust_contract/vectors/terms/private.json"),
    };
    serde_json::from_str::<Value>(vector).unwrap()["row"].clone()
}

#[test]
fn visible_but_benchmark_ineligible() {
    let policy = policy();
    for (profile, field) in [
        ("fame_v1", "certificate_holder"),
        ("lng_v1", "production_country"),
    ] {
        let mut row = typed_row(profile);
        assert!(candidate_is_benchmark_qualified(&policy, &row, &clock()));
        let terms = if profile == "fame_v1" {
            "fame_terms"
        } else {
            "lng_terms"
        };
        row[terms][field] = json!("");
        assert!(!candidate_is_benchmark_qualified(&policy, &row, &clock()));
    }
}

#[test]
fn zero_remaining_and_off_spec_are_not_benchmarks() {
    let policy = policy();
    for quantity in ["0.00", "-1.00"] {
        let mut row = typed_row("fame_v1");
        row["remaining_quantity_mt"] = json!(quantity);
        assert!(!candidate_is_benchmark_qualified(&policy, &row, &clock()));
    }
    let mut row = typed_row("fame_v1");
    row["off_spec"] = json!(true);
    assert!(!candidate_is_benchmark_qualified(&policy, &row, &clock()));
}

#[test]
fn benchmark_uses_captured_singapore_date() {
    let policy = policy();
    let mut row = typed_row("fame_v1");
    row["fame_terms"]["certificate_valid_until"] = json!("2026-10-15");
    assert!(candidate_is_benchmark_qualified(&policy, &row, &clock()));
    let after = VisibilityClock::at("2026-10-15T16:00:00Z".parse().unwrap());
    assert!(!candidate_is_benchmark_qualified(&policy, &row, &after));
}

#[test]
fn scalar_scheme_is_not_an_allowlist_and_all_fields_are_required() {
    let policy = policy();
    let product = policy
        .products
        .iter()
        .find(|product| product.qualification_profile == "scalar")
        .unwrap();
    let mut row = typed_row("fame_v1");
    row["product_id"] = json!(product.id);
    row["certification_scheme"] = json!("  custom scheme  ");
    row["carbon_intensity_gco2_mj"] = json!("0.00");
    row["origin"] = json!("test origin");
    assert!(candidate_is_benchmark_qualified(&policy, &row, &clock()));
    for field in [
        "certification_scheme",
        "specification_standard",
        "feedstock",
        "origin",
    ] {
        let mut invalid = row.clone();
        invalid[field] = json!(" \t\n");
        assert!(
            !candidate_is_benchmark_qualified(&policy, &invalid, &clock()),
            "{field}"
        );
    }
    for field in ["certification_declared", "msds_available"] {
        let mut invalid = row.clone();
        invalid[field] = json!(false);
        assert!(
            !candidate_is_benchmark_qualified(&policy, &invalid, &clock()),
            "{field}"
        );
    }
    row["carbon_intensity_gco2_mj"] = Value::Null;
    assert!(!candidate_is_benchmark_qualified(&policy, &row, &clock()));
    row["side"] = json!("BID");
    assert!(candidate_is_benchmark_qualified(&policy, &row, &clock()));
}

#[test]
fn mixed_real_demo_vwap_matches_current_python() {
    assert_eq!(
        qualified_vwap(&[(D::from(10), D::from(100)), (D::from(30), D::from(200))]),
        Some(D::from_str("175.00").unwrap())
    );
}

#[test]
fn empty_and_nonpositive_totals_have_no_price() {
    assert_eq!(qualified_vwap(&[]), None);
    assert_eq!(qualified_vwap(&[(D::ZERO, D::from(100))]), None);
    assert_eq!(qualified_vwap(&[(-D::ONE, D::from(100))]), None);
}

#[test]
fn midpoint_rounding_is_half_even_with_two_decimal_places() {
    for (price, expected) in [
        ("100.005", "100.00"),
        ("100.015", "100.02"),
        ("0.005", "0.00"),
    ] {
        let result = qualified_vwap(&[(D::ONE, D::from_str(price).unwrap())]).unwrap();
        assert_eq!(result.to_string(), expected);
    }
}

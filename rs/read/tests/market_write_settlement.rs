use rust_decimal::Decimal;
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::market_lifecycle::{Delivery, delivery_amounts, settlement_hash};

fn decimal(text: &str) -> Decimal {
    text.parse().unwrap()
}

#[test]
fn delivery_domain_is_exact_and_bounded() {
    for (quantity, price) in [("0.01", "0.01"), ("100000", "1000000"), ("3.00", "100.00")] {
        assert!(
            Delivery::parse(&json!({"final_quantity_mt":quantity,"final_price_per_mt":price}))
                .is_ok()
        );
    }
    for value in [
        json!(null),
        json!({}),
        json!({"final_quantity_mt":null,"final_price_per_mt":"100"}),
    ] {
        assert!(Delivery::parse(&value).is_err());
    }
    for invalid in [
        "0",
        "-1",
        "NaN",
        "Infinity",
        "1e2",
        "+1",
        " 1",
        "1 ",
        "-0",
        "1.000",
        "100000.01",
    ] {
        assert!(
            Delivery::parse(&json!({"final_quantity_mt":invalid,"final_price_per_mt":"100"}))
                .is_err(),
            "{invalid}"
        );
    }
    for invalid in [
        "0",
        "-1",
        "NaN",
        "Infinity",
        "1e2",
        "+1",
        " 1",
        "-0",
        "1.000",
        "1000000.01",
    ] {
        assert!(
            Delivery::parse(&json!({"final_quantity_mt":"1","final_price_per_mt":invalid}))
                .is_err(),
            "{invalid}"
        );
    }
    let exponent: serde_json::Value =
        serde_json::from_str(r#"{"final_quantity_mt":1e0,"final_price_per_mt":100}"#).unwrap();
    assert!(Delivery::parse(&exponent).is_err());
}

#[test]
fn settlement_rounds_once_half_up() {
    for (quantity, price, percent, fee, total, commission) in [
        ("0.03", "100.50", "0", Some("1.50"), "3.02", "0.05"),
        ("0.03", "100.50", "16.550", None, "3.02", "0.50"),
        ("3", "100", "1.125", None, "300", "3.38"),
        ("3", "100", "99.999", Some("0"), "300", "0"),
        (
            "100000",
            "1000000",
            "100",
            None,
            "100000000000",
            "100000000000",
        ),
    ] {
        let delivery =
            Delivery::parse(&json!({"final_quantity_mt":quantity,"final_price_per_mt":price}))
                .unwrap();
        let (actual_total, actual_fee) =
            delivery_amounts(&delivery, decimal(percent), fee.map(decimal)).unwrap();
        assert_eq!(actual_total, decimal(total));
        assert_eq!(actual_fee, decimal(commission));
    }
}

#[test]
fn settlement_hash_normalizes_amounts_and_binds_both_fields() {
    let trade = Uuid::from_u128(8);
    let parse =
        |q, p| Delivery::parse(&json!({"final_quantity_mt":q,"final_price_per_mt":p})).unwrap();
    assert_eq!(
        settlement_hash(trade, Some(&parse("3", "100"))).unwrap(),
        settlement_hash(trade, Some(&parse("3.00", "100.00"))).unwrap()
    );
    assert_ne!(
        settlement_hash(trade, Some(&parse("3", "100"))).unwrap(),
        settlement_hash(trade, Some(&parse("2", "100"))).unwrap()
    );
    assert_ne!(
        settlement_hash(trade, None).unwrap(),
        settlement_hash(trade, Some(&parse("3", "100"))).unwrap()
    );
}

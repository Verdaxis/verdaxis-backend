use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use verdaxis_read::routes::curves_legacy::{curve_csv, midpoint};

#[test]
fn midpoint_requires_both_sides_and_rounds_half_even() {
    let d = |s| Decimal::from_str(s).unwrap();
    assert_eq!(
        midpoint(Some(d("900.00")), Some(d("950.00"))),
        Some(d("925.00"))
    );
    assert_eq!(midpoint(Some(d("1.00")), Some(d("1.01"))), Some(d("1.00")));
    assert_eq!(midpoint(Some(d("1.01")), Some(d("1.02"))), Some(d("1.02")));
    assert_eq!(midpoint(Some(d("900")), None), None);
    assert_eq!(midpoint(None, Some(d("950"))), None);
    assert_eq!(midpoint(None, None), None);
}

#[test]
fn csv_has_exact_python_header_nulls_scale_quoting_and_lf() {
    let header = "availability_window,best_bid,best_ask,mid_price,spread,volume_mt,order_count\n";
    assert_eq!(curve_csv(&[]).unwrap(), header.as_bytes());
    let points = [
        json!({"availability_window":"SPOT", "best_bid":"900.00", "best_ask":null,
        "mid_price":null,"spread":null,"volume_mt":"500.000","order_count":1}),
        json!({"availability_window":"a,\"b\n\rc", "best_bid":null,"best_ask":"1.00",
        "mid_price":null,"spread":null,"volume_mt":"0","order_count":2}),
    ];
    assert_eq!(
        curve_csv(&points).unwrap(),
        format!("{header}SPOT,900.00,,,,500.000,1\n\"a,\"\"b\n\rc\",,1.00,,,0,2\n").as_bytes()
    );
}

#[test]
fn stored_price_text_survives_alias_selection_and_csv() {
    use verdaxis_read::routes::curves_legacy::{CurveAggregate, curve_points};
    let bid = format!("1.{}", "0".repeat(30));
    let ask = format!("1.01{}", "0".repeat(28));
    let row = |window: &str, side: &str, price: &str| CurveAggregate {
        window: window.into(),
        side: side.into(),
        max_price: Some(price.into()),
        min_price: Some(price.into()),
        volume: Decimal::from_str("100.00").unwrap(),
        count: 1,
    };
    let points = curve_points(
        vec![
            row("Q1_2026", "BID", "0.99"),
            row("2026-Q1", "BID", &bid),
            row("Q1 2026", "ASK", &ask),
        ],
        chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(),
    )
    .unwrap();
    assert_eq!(points[0]["best_bid"], bid);
    assert_eq!(points[0]["best_ask"], ask);
    assert_eq!(points[0]["mid_price"], "1.00");
    assert_eq!(points[0]["spread"], "0.01");
    let csv = String::from_utf8(curve_csv(&points).unwrap()).unwrap();
    assert!(csv.contains(&format!("2026-Q1,{bid},{ask},1.00,0.01,300.00,3\n")));
}

#[test]
fn aliases_merge_before_midpoint_volume_count_and_window_sort() {
    use verdaxis_read::routes::curves_legacy::{CurveAggregate, curve_points};
    let row = |window: &str, side: &str, price: &str, volume: &str, count| CurveAggregate {
        window: window.into(),
        side: side.into(),
        max_price: Some(price.to_owned()),
        min_price: Some(price.to_owned()),
        volume: Decimal::from_str(volume).unwrap(),
        count,
    };
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let points = curve_points(
        vec![
            row("Q1_2026", "BID", "900.00", "100.000", 1),
            row("2026-Q1", "BID", "910.00", "200.000", 2),
            row("Q1 2026", "ASK", "950.00", "300.000", 3),
            row("Spot", "ASK", "999.00", "50.000", 1),
        ],
        today,
    )
    .unwrap();
    assert_eq!(points[0]["availability_window"], "SPOT");
    assert_eq!(points[0]["mid_price"], serde_json::Value::Null);
    assert_eq!(
        points[1],
        json!({"availability_window":"2026-Q1", "best_bid":"910.00",
        "best_ask":"950.00", "mid_price":"930.00", "spread":"40.00",
        "volume_mt":"600.000", "order_count":6})
    );
    assert!(curve_points(vec![], today).unwrap().is_empty());
}

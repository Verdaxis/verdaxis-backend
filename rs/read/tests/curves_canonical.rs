use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use verdaxis_read::routes::curves_canonical::curve_projection::{
    Book, default_windows, midpoint, money,
};
use verdaxis_read::routes::curves_canonical::{benchmarks, curve_projection::cells};

#[test]
fn canonical_midpoint_requires_both_sides_and_rounds_half_even() {
    let decimal = |value| Decimal::from_str(value).unwrap();
    assert_eq!(
        midpoint(Some(decimal("1.00")), Some(decimal("1.01"))),
        Some(decimal("1.00"))
    );
    assert_eq!(
        midpoint(Some(decimal("1.01")), Some(decimal("1.02"))),
        Some(decimal("1.02"))
    );
    assert_eq!(midpoint(Some(decimal("1.00")), None), None);
    assert_eq!(money(decimal("1.005")), "1.00");
    assert_eq!(money(decimal("1.015")), "1.02");
    assert_eq!(money(decimal("-0.005")), "-0.00");
}

#[test]
fn sparse_keys_cannot_expand_to_cross_product() {
    use verdaxis_read::routes::curves_canonical::signals::curve_keys::complete_grid;
    let keys = json!([
        {"market_product":"first","delivery_point_id":"one","availability_window":"SPOT"},
        {"market_product":"second","delivery_point_id":"two","availability_window":"SPOT"}
    ]);
    assert!(!complete_grid(&keys));
    let mut grid = keys.as_array().unwrap().clone();
    grid.push(
        json!({"market_product":"first","delivery_point_id":"two","availability_window":"SPOT"}),
    );
    grid.push(
        json!({"market_product":"second","delivery_point_id":"one","availability_window":"SPOT"}),
    );
    assert!(complete_grid(&json!(grid)));
}

#[test]
fn unknown_metadata_never_supplies_economics() {
    let now = "2026-10-15T15:59:59Z".parse().unwrap();
    let book = Book {
        unknown_count: 9,
        ..Default::default()
    };
    let (cell, _) = cells::values(&book, None, None, &json!({}), &json!(null), "scalar", now);
    assert_eq!(cell["unknown_order_count"], 9);
    assert_eq!(cell["demo_status"], "UNKNOWN");
    assert_eq!(cell["primary_value"], json!(null));
    assert_eq!(cell["volume_mt"], "0");
    assert_eq!(cell["order_count"], 0);
}

#[test]
fn real_order_evidence_is_not_blended_with_demo() {
    let now = "2026-10-15T15:59:59Z".parse().unwrap();
    let mut book = Book::default();
    for (side, real, price) in [
        ("BID", true, 100),
        ("ASK", true, 120),
        ("BID", false, 900),
        ("ASK", false, 950),
    ] {
        book.add(
            side,
            real,
            Decimal::from(price),
            Decimal::from(10),
            Some(now),
            false,
        );
    }
    let (cell, _) = cells::values(&book, None, None, &json!({}), &json!(null), "scalar", now);
    assert_eq!(cell["primary_value"], "110.00");
    assert_eq!(cell["volume_mt"], "20.00");
    assert_eq!(cell["real_order_count"], 2);
    assert_eq!(cell["demo_order_count"], 2);
    assert_eq!(cell["demo_status"], "REAL_ONLY");
}

#[test]
fn managed_demo_midpoint_keeps_historical_trade_metadata() {
    let now = "2026-10-15T15:59:59Z".parse().unwrap();
    let mut book = Book::default();
    book.add(
        "BID",
        false,
        Decimal::from(100),
        Decimal::ONE,
        Some(now),
        true,
    );
    book.add(
        "ASK",
        false,
        Decimal::from(120),
        Decimal::ONE,
        Some(now),
        true,
    );
    let trade = json!({"price_per_mt_usd":"75.00","demo_status":"DEMO_ONLY","confirmed_at":"2026-10-01T00:00:00Z"});
    let (cell, _) = cells::values(
        &book,
        Some(&trade),
        None,
        &json!({}),
        &json!(null),
        "scalar",
        now,
    );
    assert_eq!(cell["primary_value"], "110.00");
    assert_eq!(cell["public_source_label"], "Demo orderbook midpoint");
}

#[test]
fn freshness_boundary_and_no_data_are_distinct() {
    let now = "2026-10-15T16:00:00Z".parse().unwrap();
    for (observed, expected) in [
        ("2026-10-08T16:00:00Z", "FRESH"),
        ("2026-10-08T15:59:59.999999Z", "STALE"),
    ] {
        let benchmark = json!({"price":"100.00","observed_at":observed});
        let (cell, _) = cells::values(
            &Book::default(),
            None,
            Some(&benchmark),
            &json!({}),
            &json!(null),
            "scalar",
            now,
        );
        assert_eq!(cell["staleness_status"], expected);
    }
}

#[test]
fn canonical_and_board_horizons_remain_distinct() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let canonical = default_windows(date, false);
    let board = default_windows(date, true);
    assert_eq!(canonical.len(), 24);
    assert_eq!(canonical[1], "2026-10");
    assert_eq!(board.len(), 13);
    assert_eq!(board[1], "2026-11");
    assert_eq!(board.last().unwrap(), "2028-CAL");
    assert!(
        !benchmarks::policy()["seed_quotes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

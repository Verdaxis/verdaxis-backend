//! Task 7c pure projection checks.

#[test]
fn listing_snapshot_keeps_supplied_whitespace() {
    use verdaxis_read::market::terms::public_response_terms;
    let base: serde_json::Value = serde_json::from_str(r####"{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"}"####).unwrap();
    for whitespace in [
        " ", "\t\n", "\u{a0}", "\u{2003}", "\u{3000}", "\u{1c}", "\u{1d}", "\u{1e}", "\u{1f}",
    ] {
        let mut terms = base.clone();
        terms["standard_edition"] = serde_json::json!(format!(
            "{whitespace}{}{whitespace}",
            base["standard_edition"].as_str().unwrap()
        ));
        assert_eq!(
            public_response_terms("fame_v1", &terms).unwrap()["standard_edition"],
            terms["standard_edition"]
        );
    }
}

#[test]
fn map_decimal_matches_python_context_and_exact_comparison() {
    use verdaxis_read::routes::orderbook_map::map_decimal;
    let vectors = [
        ("0", "0.00"),
        ("1E-29", "2E-29"),
        ("10.00", "3.000"),
        ("12345678901234567890123456789", "1"),
        ("99999999999999999999999999999", "1"),
        ("1E+40", "9E+39"),
        ("0.00000001", "0.000000001"),
    ];
    let expected: serde_json::Value = serde_json::json!([
        ["0.00", 0],
        ["3E-29", -1],
        ["13.000", 1],
        ["1.234567890123456789012345679E+28", 1],
        ["1.000000000000000000000000000E+29", 1],
        ["1.9E+40", 1],
        ["1.1E-8", 1]
    ]);
    for ((left, right), result) in vectors.iter().zip(expected.as_array().unwrap()) {
        assert_eq!(
            map_decimal::compare(
                &map_decimal::add(left, right).unwrap(),
                result[0].as_str().unwrap()
            )
            .unwrap(),
            std::cmp::Ordering::Equal
        );
        let comparison = map_decimal::compare(left, right).unwrap();
        assert_eq!(comparison as i8, result[1].as_i64().unwrap() as i8);
    }
}
use serde_json::json;
use verdaxis_read::routes::orderbook_map::{compact_groups, product_counts};

#[test]
fn compact_totals_keep_python_precision_and_timestamp_order() {
    let base = json!({"product_id":"p","product_name":"Product","market_product":"PRODUCT",
        "fuel_type":"fuel","delivery_point_id":"d","delivery_point_name":"Point","region":"region",
        "evidence_class":"REAL","source_kind":"LIVE_ORDER","scope":"DELIVERY_POINT","demo_status":"REAL_ONLY",
        "side":"ASK","availability_window":"SPOT","min_price":"1E-29","max_price":"100000000000000000000000000000",
        "total_quantity":"12345678901234567890123456789","order_count":1,"observed_at":"2026-10-15T12:00:00Z"});
    let mut later = base.clone();
    later["observed_at"] = json!("2026-10-15T12:00:00.000001Z");
    later["total_quantity"] = json!("1");
    let (markets, _) = compact_groups(&[base, later], "2026-10-15".parse().unwrap()).unwrap();
    assert_eq!(
        verdaxis_read::routes::orderbook_map::map_decimal::compare(
            markets[0]["ask_total_quantity"].as_str().unwrap(),
            "1.234567890123456789012345679E+28"
        )
        .unwrap(),
        std::cmp::Ordering::Equal
    );
    assert_eq!(markets[0]["observed_at"], "2026-10-15T12:00:00.000001Z");
}

#[test]
fn compact_keeps_evidence_classes_and_nearest_forward_demo_sides() {
    let base = json!({"product_id":"p","product_name":"Product","market_product":"PRODUCT",
        "fuel_type":"fuel","delivery_point_id":"d","delivery_point_name":"Point","region":"region",
        "evidence_class":"DEMO","source_kind":"DEMO_SEED","scope":"DELIVERY_POINT","demo_status":"DEMO_ONLY",
        "side":"ASK","availability_window":"2026-11","min_price":"10.00","max_price":"20.00",
        "total_quantity":"30.00","order_count":2,"observed_at":"2026-10-15T12:00:00Z"});
    let mut bid = base.clone();
    bid["side"] = json!("BID");
    let mut distant = base.clone();
    distant["availability_window"] = json!("2027-01");
    let mut real = base.clone();
    real["evidence_class"] = json!("REAL");
    real["source_kind"] = json!("LIVE_ORDER");
    real["demo_status"] = json!("REAL_ONLY");
    let (markets, demos) =
        compact_groups(&[distant, base, bid, real], "2026-10-15".parse().unwrap()).unwrap();
    assert_eq!(markets.len(), 2);
    for (index, expected) in [(0, "60"), (1, "30")] {
        assert_eq!(
            verdaxis_read::routes::orderbook_map::map_decimal::compare(
                markets[index]["ask_total_quantity"].as_str().unwrap(),
                expected
            )
            .unwrap(),
            std::cmp::Ordering::Equal
        );
    }
    assert!(markets[0]["spot_best_ask"].is_null());
    assert_eq!(demos.len(), 2);
    assert_eq!(demos[0]["side"], "ASK");
    assert_eq!(demos[1]["side"], "BID");
    assert!(
        demos
            .iter()
            .all(|group| group["availability_window"] == "2026-11")
    );
}

#[test]
fn counts_keep_zero_products_and_ignore_unknown_codes() {
    let codes = vec!["first".into(), "second".into()];
    let counts = product_counts(&codes, &[("second".into(), 7), ("unknown".into(), 9)]);
    assert_eq!(counts.len(), 2);
    assert_eq!(counts["first"], 0);
    assert_eq!(counts["second"], 7);
}

#[test]
fn page_aliases_override_skip_and_limit() {
    use verdaxis_read::routes::orderbook_lists::page_offset;
    assert_eq!(page_offset(99, 50, Some(2), Some(3)), Some((3, 3)));
    assert_eq!(page_offset(7, 50, None, Some(3)), Some((7, 3)));
    assert_eq!(page_offset(7, 50, Some(1), None), Some((0, 50)));
}

#[test]
fn page_offset_rejects_zero_page_and_overflow() {
    use verdaxis_read::routes::orderbook_lists::page_offset;
    assert_eq!(page_offset(0, 50, Some(0), None), None);
    assert_eq!(page_offset(0, 100, Some(u64::MAX), None), None);
}

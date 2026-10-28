//! Task 7c pure projection checks.

#[test]
fn listing_fame_normalization_matches_python_without_changing_snapshot_projection() {
    use verdaxis_read::market::terms::{public_response_terms, redact_terms};
    let script = r#"
import json
from scripts.rust_contract.export_terms_vectors import _fame_private_terms
from app.services.fame_order import public_fame_terms
vectors = []
for whitespace in [' ', '\t\n', ' ', ' ', '　', '\x1c', '\x1d', '\x1e', '\x1f']:
    terms = _fame_private_terms()
    for field in ['standard_edition', 'en_climate_class', 'production_origin',
                  'feedstock_origin', 'shipping_location', 'ci_methodology', 'ci_boundary']:
        terms[field] = whitespace + terms[field] + whitespace
    vectors.append([terms, public_fame_terms(terms)])
print(json.dumps(vectors))
"#;
    let output = std::process::Command::new(
        std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()),
    )
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .env("ENVIRONMENT", "test")
    .env("RELEASE_SHA", "test")
    .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
    .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
    .args(["-c", script])
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let vectors: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for vector in vectors.as_array().unwrap() {
        assert_eq!(redact_terms("fame_v1", &vector[0]).unwrap(), vector[1]);
        assert_eq!(
            public_response_terms("fame_v1", &vector[0]).unwrap()["standard_edition"],
            vector[0]["standard_edition"]
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
    let output = std::process::Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
        .args(["-c", "import json,sys; from decimal import Decimal; print(json.dumps([[str(Decimal(a)+Decimal(b)), (Decimal(a)>Decimal(b))-(Decimal(a)<Decimal(b))] for a,b in json.loads(sys.argv[1])]))"])
        .arg(serde_json::to_string(&vectors).unwrap()).output().unwrap();
    assert!(output.status.success());
    let expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
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

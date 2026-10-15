//! Task 6: shared visibility, decimal, digest, and public projection rules.
//!
//! The pure vectors live here. The PostgreSQL visibility and trade-evidence
//! comparison against the Python oracle lives in `market_contract_postgres.rs`
//! and is gated by the `contract-tests` feature, so this target always builds
//! and runs without a database.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use uuid::Uuid;

use verdaxis_read::{
    Json, MarketPolicy, PointPolicy, ProductPolicy, QualificationInput, canonical_terms_from_row,
    decimal_text, digest_terms, normalize_certification_scheme, normalize_window,
    order_is_execution_qualified_at, public_order_sql, redact_terms, trade_evidence_sql,
    validate_fame_terms, validate_lng_terms, window_display_label, window_sort_key,
    window_start_date,
};

const PRIVATE_VECTOR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/rust_contract/vectors/terms/private.json"
));
const NON_ASCII_VECTOR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/rust_contract/vectors/terms/non_ascii.json"
));
const FAME_VECTOR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/rust_contract/vectors/terms/fame.json"
));

fn vector(text: &str) -> Json {
    serde_json::from_str(text).expect("a committed terms vector must be valid JSON")
}

// ---------------------------------------------------------------------------
// Reviewed-terms digest and decimal text (Review Focus 2).
// ---------------------------------------------------------------------------

#[test]
fn private_terms_digest_matches_python() {
    let vector = vector(PRIVATE_VECTOR);
    assert_eq!(
        canonical_terms_from_row(&vector["row"]).unwrap(),
        vector["canonical_terms"]
    );
    assert_eq!(
        digest_terms(&vector["canonical_terms"]).unwrap(),
        vector["digest"]
    );
    let public = redact_terms("lng_v1", &vector["lng_terms"]).unwrap();
    assert!(public.get("scheme_certificate_number").is_none());
    assert!(public.get("physical_supplier_name").is_none());
    assert_eq!(public.get("reseller"), Some(&json!(true)));
    assert_eq!(
        public.get("fueleu_recognition"),
        Some(&json!("UNION_DATABASE"))
    );
    assert!(public.get("bio_share_pct").is_some());
}

#[test]
fn non_ascii_terms_digest_matches_python() {
    let vector = vector(NON_ASCII_VECTOR);
    assert_eq!(
        canonical_terms_from_row(&vector["row"]).unwrap(),
        vector["canonical_terms"]
    );
    assert_eq!(
        digest_terms(&vector["canonical_terms"]).unwrap(),
        vector["digest"]
    );
}

#[test]
fn fame_terms_digest_matches_python() {
    let vector = vector(FAME_VECTOR);
    assert_eq!(
        canonical_terms_from_row(&vector["row"]).unwrap(),
        vector["canonical_terms"]
    );
    assert_eq!(
        digest_terms(&vector["canonical_terms"]).unwrap(),
        vector["digest"]
    );
    let public = redact_terms("fame_v1", &vector["fame_terms"]).unwrap();
    for secret in [
        "batch_reference",
        "producing_site",
        "certificate_reference",
        "certificate_holder",
        "certificate_scope",
        "document_references",
    ] {
        assert!(public.get(secret).is_none(), "{secret} must not be public");
    }
    assert_eq!(public["side"], json!("ASK"));
    assert_eq!(public["evidence_due"], json!("BEFORE_LOADING"));
    assert_eq!(
        public["quality_evidence"],
        json!({
            "status": "AVAILABLE",
            "results": [{"property": "WATER_MG_KG", "value": "120", "method": "EN ISO 12937"}],
        })
    );
    assert_eq!(
        public["sustainability_evidence"],
        json!({"status": "AVAILABLE", "document_type": "POS", "due": "BEFORE_LOADING"})
    );
}

#[test]
fn digest_is_stable_across_key_order() {
    let left = json!({"b": 1, "a": {"y": "2", "x": "1"}});
    let right = json!({"a": {"x": "1", "y": "2"}, "b": 1});
    assert_eq!(digest_terms(&left).unwrap(), digest_terms(&right).unwrap());
    assert_eq!(digest_terms(&left).unwrap().len(), 64);
}

#[test]
fn zero_and_null_stay_distinct() {
    assert_eq!(decimal_text(Decimal::ZERO), "0");
    assert_ne!(serde_json::json!("0.00"), serde_json::Value::Null);
}

#[test]
fn decimal_text_normalizes_scale_but_never_zero() {
    assert_eq!(
        decimal_text(Decimal::from_str_exact("500.0000").unwrap()),
        "500"
    );
    assert_eq!(
        decimal_text(Decimal::from_str_exact("125.5000").unwrap()),
        "125.5"
    );
    assert_eq!(decimal_text(Decimal::from_str_exact("0.00").unwrap()), "0");
    assert_eq!(decimal_text(Decimal::from_str_exact("-0.00").unwrap()), "0");
    assert_eq!(
        decimal_text(Decimal::from_str_exact("1180.50").unwrap()),
        "1180.5"
    );
}

#[test]
fn verification_scheme_normalizes_like_python() {
    assert_eq!(
        normalize_certification_scheme(Some("  iscc eu ")),
        Some("ISCC EU".into())
    );
    assert_eq!(normalize_certification_scheme(Some("   ")), None);
    assert_eq!(normalize_certification_scheme(None), None);
}

// ---------------------------------------------------------------------------
// Availability windows (app/services/availability_windows.py).
// ---------------------------------------------------------------------------

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

#[test]
fn window_aliases_and_grammar_match_python() {
    for (input, expected) in [
        ("SPOT", "SPOT"),
        ("Spot", "SPOT"),
        ("spot", "SPOT"),
        (" 2025-03 ", "2025-03"),
        ("2025-Q2", "2025-Q2"),
        ("2025-CAL", "2025-CAL"),
        ("Q1_2025", "2025-Q1"),
        ("Q4 2026", "2026-Q4"),
        ("FORWARD_2027", "2027-CAL"),
        ("Forward 2028", "2028-CAL"),
    ] {
        assert_eq!(
            normalize_window(input, today()).unwrap(),
            expected,
            "{input}"
        );
    }
    for invalid in [
        "", "   ", "2025-13", "2025-Q5", "2025-1", "2025", "q1_2025", "2025-00",
    ] {
        assert!(
            normalize_window(invalid, today()).is_err(),
            "{invalid:?} must fail"
        );
    }
}

#[test]
fn window_sort_keys_and_labels_match_python() {
    let today = today();
    assert_eq!(window_sort_key("SPOT", today).unwrap(), (0, today, 0));
    assert_eq!(
        window_sort_key("2025-03", today).unwrap(),
        (1, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), 0)
    );
    assert_eq!(
        window_sort_key("2025-Q2", today).unwrap(),
        (1, NaiveDate::from_ymd_opt(2025, 4, 1).unwrap(), 1)
    );
    assert_eq!(
        window_sort_key("2025-CAL", today).unwrap(),
        (1, NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), 2)
    );
    assert_eq!(
        window_start_date("2025-Q3", today).unwrap(),
        NaiveDate::from_ymd_opt(2025, 7, 1).unwrap()
    );
    for (input, expected) in [
        ("SPOT", "Spot"),
        ("2025-03", "Mar 2025"),
        ("2025-Q2", "Q2 2025"),
        ("2025-CAL", "CAL 2025"),
    ] {
        assert_eq!(window_display_label(input).unwrap(), expected);
    }
}

// ---------------------------------------------------------------------------
// Typed execution qualification (app/services/execution_policy.py:112-150).
// ---------------------------------------------------------------------------

#[test]
fn trailing_zero_declarations_match_python() {
    let cases = vector(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/rust_contract/vectors/terms/decimal_validation.json"
    )));
    for case in cases.as_array().unwrap() {
        let terms = &case["terms"];
        let result = match case["profile"].as_str().unwrap() {
            "lng_v1" => validate_lng_terms("ASK", Some(terms)),
            _ => validate_fame_terms("ASK", Some(terms), today()),
        };
        assert!(result.is_ok(), "Python accepts {case}: {result:?}");
    }
}

#[test]
fn public_quality_wire_decimals_match_python() {
    assert_public_vectors(true);
}

#[test]
fn public_bid_defaults_match_python() {
    assert_public_vectors(false);
}

fn assert_public_vectors(quality: bool) {
    let cases = vector(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/rust_contract/vectors/terms/public_terms.json"
    )));
    for case in cases.as_array().unwrap() {
        if case["name"].as_str().unwrap().starts_with("quality-") != quality {
            continue;
        }
        assert_eq!(
            redact_terms(case["profile"].as_str().unwrap(), &case["terms"]).unwrap(),
            case["public"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn large_decimal_digest_matches_python() {
    let cases = vector(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/rust_contract/vectors/terms/large_decimals.json"
    )));
    for case in cases.as_array().unwrap() {
        let canonical = canonical_terms_from_row(&case["row"]).unwrap();
        assert_eq!(canonical, case["canonical_terms"]);
        assert_eq!(digest_terms(&canonical).unwrap(), case["digest"]);
    }
    let mut invalid = vector(PRIVATE_VECTOR)["row"].clone();
    invalid["carbon_intensity_gco2_mj"] = json!("not a decimal");
    assert!(canonical_terms_from_row(&invalid).is_err());
}

#[test]
fn window_edge_inputs_match_python() {
    let cases = vector(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/rust_contract/vectors/terms/windows.json"
    )));
    for case in cases.as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        assert_eq!(
            normalize_window(input, today()).unwrap(),
            case["normalized"]
        );
        assert_eq!(window_display_label(input).unwrap(), case["label"]);
        let start = window_start_date(input, today());
        if case["start"].is_null() {
            assert!(start.is_err());
        } else {
            assert_eq!(start.unwrap().format("%Y-%m-%d").to_string(), case["start"]);
        }
    }
}

#[test]
fn qualification_clock_matches_python() {
    let cases = vector(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/rust_contract/vectors/terms/qualification_clock.json"
    )));
    let policy = fixture_policy();
    let point = id(POINT_A).to_string();
    for case in cases.as_array().unwrap() {
        let now = case["instant"].as_str().unwrap().parse().unwrap();
        let clock = verdaxis_read::VisibilityClock::at(now);
        assert_eq!(
            order_is_execution_qualified_at(
                &policy,
                &fame_input(&point, Some(&case["terms"])),
                clock.singapore_date
            ),
            case["qualified"]
        );
    }
}

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

const SCALAR_PRODUCT: u128 = 1;
const FAME_PRODUCT: u128 = 2;
const LNG_PRODUCT: u128 = 3;
const POINT_A: u128 = 10;
const POINT_B: u128 = 11;

fn product(
    n: u128,
    code: &str,
    fuel_type: &str,
    fuel_grade: &str,
    profile: &str,
    lanes: Vec<Uuid>,
) -> ProductPolicy {
    ProductPolicy {
        id: id(n),
        code: code.to_owned(),
        name: format!("Fixture {code}"),
        fuel_type: fuel_type.to_owned(),
        fuel_grade: fuel_grade.to_owned(),
        execution_mode: "ORDERBOOK".to_owned(),
        lanes,
        qualification_profile: profile.to_owned(),
        display_order: n as u32,
    }
}

fn fixture_policy() -> MarketPolicy {
    MarketPolicy {
        source_sha: "a".repeat(40),
        products: vec![
            product(
                SCALAR_PRODUCT,
                "FIXTURE_SCALAR",
                "Methanol",
                "Bio",
                "scalar",
                vec![id(POINT_A)],
            ),
            product(
                FAME_PRODUCT,
                "FIXTURE_FAME",
                "FAME",
                "UCOME",
                "fame_v1",
                vec![id(POINT_A)],
            ),
            product(
                LNG_PRODUCT,
                "FIXTURE_LNG",
                "LNG",
                "Bio",
                "lng_v1",
                vec![id(POINT_B)],
            ),
        ],
        points: vec![
            PointPolicy {
                id: id(POINT_A),
                name: "Fixture point A".to_owned(),
                region: "Asia".to_owned(),
                timezone: "Asia/Singapore".to_owned(),
                display_order: 1,
            },
            PointPolicy {
                id: id(POINT_B),
                name: "Fixture point B".to_owned(),
                region: "Europe".to_owned(),
                timezone: "Europe/Amsterdam".to_owned(),
                display_order: 2,
            },
        ],
    }
}

fn scalar_input<'a>(point: &'a str) -> QualificationInput<'a> {
    QualificationInput {
        product_id: Some(id(SCALAR_PRODUCT)),
        market_product: None,
        side: "BID",
        delivery_point_id: Some(point),
        availability_window: "SPOT",
        off_spec: false,
        msds_available: false,
        certification_scheme: None,
        certification_declared: false,
        fame_terms: None,
        lng_terms: None,
    }
}

fn valid_fame_ask() -> Value {
    json!({
        "schema_version": 1,
        "side": "ASK",
        "neat_fame": true,
        "nomination_status": "PENDING",
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "EN 14214:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "CERT-1",
        "certificate_holder": "Jurong Renewables",
        "certificate_valid_until": "2027-06-30",
        "evidence_status": "DECLARED",
        "document_references": [],
        "evidence_due": "BEFORE_LOADING"
    })
}

#[test]
fn scalar_bid_needs_no_declaration_but_ask_does() {
    let policy = fixture_policy();
    let point = id(POINT_A).to_string();
    let today = today();

    assert!(order_is_execution_qualified_at(
        &policy,
        &scalar_input(&point),
        today
    ));

    let mut ask = scalar_input(&point);
    ask.side = "ASK";
    assert!(!order_is_execution_qualified_at(&policy, &ask, today));

    let mut declared = scalar_input(&point);
    declared.side = "ASK";
    declared.certification_scheme = Some("  iscc eu ");
    declared.certification_declared = true;
    assert!(order_is_execution_qualified_at(&policy, &declared, today));

    let mut undeclared = scalar_input(&point);
    undeclared.side = "ASK";
    undeclared.certification_scheme = Some("ISCC EU");
    assert!(!order_is_execution_qualified_at(
        &policy,
        &undeclared,
        today
    ));
}

#[test]
fn off_spec_is_never_execution_qualified() {
    let policy = fixture_policy();
    let point = id(POINT_A).to_string();
    let mut order = scalar_input(&point);
    order.off_spec = true;
    assert!(!order_is_execution_qualified_at(&policy, &order, today()));
}

#[test]
fn an_unknown_product_is_qualified_by_the_scalar_rule() {
    let policy = fixture_policy();
    let point = id(POINT_A).to_string();
    let mut order = scalar_input(&point);
    order.product_id = Some(id(9999));
    assert!(order_is_execution_qualified_at(&policy, &order, today()));
}

fn lng_input<'a>(side: &'a str, point: &'a str, terms: Option<&'a Json>) -> QualificationInput<'a> {
    QualificationInput {
        product_id: Some(id(LNG_PRODUCT)),
        market_product: None,
        side,
        delivery_point_id: Some(point),
        availability_window: "SPOT",
        off_spec: false,
        msds_available: true,
        certification_scheme: None,
        certification_declared: false,
        fame_terms: None,
        lng_terms: terms,
    }
}

fn fame_input<'a>(point: &'a str, terms: Option<&'a Json>) -> QualificationInput<'a> {
    QualificationInput {
        product_id: Some(id(FAME_PRODUCT)),
        market_product: None,
        side: "ASK",
        delivery_point_id: Some(point),
        availability_window: "SPOT",
        off_spec: false,
        msds_available: true,
        certification_scheme: Some("ISCC EU"),
        certification_declared: true,
        fame_terms: terms,
        lng_terms: None,
    }
}

#[test]
fn lng_profile_requires_lane_msds_and_declarations() {
    let policy = fixture_policy();
    let lane = id(POINT_B).to_string();
    let wrong_lane = id(POINT_A).to_string();
    let today = today();
    let bid = json!({
        "side": "BID",
        "accepted_schemes": ["ISCC_EU"],
        "accepted_delivery_methods": ["SHIP_TO_SHIP"]
    });

    // A BID in the approved lane with a valid declaration qualifies.
    assert!(order_is_execution_qualified_at(
        &policy,
        &lng_input("BID", &lane, Some(&bid)),
        today
    ));
    // The other approved delivery point is outside the product's lane.
    assert!(!order_is_execution_qualified_at(
        &policy,
        &lng_input("BID", &wrong_lane, Some(&bid)),
        today
    ));
    // An ASK without MSDS evidence never qualifies.
    let mut no_msds = lng_input("ASK", &lane, Some(&bid));
    no_msds.msds_available = false;
    assert!(!order_is_execution_qualified_at(&policy, &no_msds, today));
    // Missing or invalid declarations fail closed.
    assert!(!order_is_execution_qualified_at(
        &policy,
        &lng_input("BID", &lane, None),
        today
    ));
    let invalid = json!({
        "side": "BID",
        "accepted_schemes": [],
        "accepted_delivery_methods": ["SHIP_TO_SHIP"]
    });
    assert!(!order_is_execution_qualified_at(
        &policy,
        &lng_input("BID", &lane, Some(&invalid)),
        today
    ));
}

#[test]
fn fame_profile_requires_typed_declarations_and_a_current_certificate() {
    let policy = fixture_policy();
    let point = id(POINT_A).to_string();
    let today = today();

    let declaration = valid_fame_ask();
    assert!(order_is_execution_qualified_at(
        &policy,
        &fame_input(&point, Some(&declaration)),
        today
    ));

    // A certificate that expired before today fails.
    let mut expired = valid_fame_ask();
    expired["certificate_valid_until"] = json!("2026-01-01");
    assert!(!order_is_execution_qualified_at(
        &policy,
        &fame_input(&point, Some(&expired)),
        today
    ));

    // A declaration missing a required field fails closed.
    let incomplete = json!({
        "side": "ASK",
        "neat_fame": true,
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "EN 14214:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_holder": "Jurong Renewables",
        "certificate_valid_until": "2027-06-30",
        "evidence_status": "DECLARED",
        "document_references": [],
        "evidence_due": "BEFORE_LOADING"
    });
    assert!(!order_is_execution_qualified_at(
        &policy,
        &fame_input(&point, Some(&incomplete)),
        today
    ));
}

#[test]
fn typed_validators_reject_a_side_mismatch_and_an_unassigned_country() {
    let terms = json!({
        "side": "BID",
        "accepted_schemes": ["ISCC_EU"],
        "accepted_delivery_methods": ["SHIP_TO_SHIP"]
    });
    assert!(validate_lng_terms("BID", Some(&terms)).is_ok());
    assert!(validate_lng_terms("ASK", Some(&terms)).is_err());

    let ask = json!({
        "side": "ASK",
        "sustainability_scheme": "ISCC_EU",
        "chain_of_custody": "MASS_BALANCE",
        "union_database_registered": true,
        "bio_share_pct": "5.00",
        "feedstock": "MANURE",
        "production_country": "NL",
        "wtt_ghg_gco2e_mj": "1.00",
        "specification_standard": "ISO 23306:2020",
        "delivery_method": "SHIP_TO_SHIP",
        "seller_is_physical_supplier": true,
        "pos_timing": "WITH_DELIVERY",
        "scheme_certificate_number": "ISCC-1",
        "physical_supplier_name": "Fixture Supplier"
    });
    assert!(validate_lng_terms("ASK", Some(&ask)).is_ok());
    let mut bad_country = ask.clone();
    bad_country["production_country"] = json!("XX");
    assert!(validate_lng_terms("ASK", Some(&bad_country)).is_err());

    let fame = valid_fame_ask();
    assert!(validate_fame_terms("ASK", Some(&fame), today()).is_ok());
    assert!(validate_fame_terms("BID", Some(&fame), today()).is_err());

    // A BID declaration carries no supplier identity, so the public projection
    // is the declaration itself. Python returns the same validated BID model.
    let bid = json!({
        "side": "BID",
        "neat_fame": true,
        "standard": "EN_14214",
        "standard_edition": "EN 14214:2019",
        "sustainability_scheme": "ISCC_EU"
    });
    assert!(validate_fame_terms("BID", Some(&bid), today()).is_ok());
    assert_eq!(
        redact_terms("fame_v1", &bid).unwrap()["require_quality_evidence"],
        json!(false)
    );
    let lng_bid = json!({
        "side": "BID",
        "accepted_schemes": ["ISCC_EU"],
        "accepted_delivery_methods": ["SHIP_TO_SHIP"]
    });
    assert_eq!(
        redact_terms("lng_v1", &lng_bid).unwrap()["require_union_database"],
        json!(true)
    );
}

// ---------------------------------------------------------------------------
// The bound statements (Review Focus 3: a permit covers all database work).
// ---------------------------------------------------------------------------

#[test]
fn bound_statements_are_parameterized_and_named_only_by_policy() {
    for sql in [public_order_sql(), trade_evidence_sql()] {
        assert!(sql.contains("$1"), "the policy must bind as a parameter");
        assert!(sql.contains("policy_products"));
        assert!(
            !sql.contains(';'),
            "each statement must be a single statement"
        );
        assert!(
            !sql.contains("format!("),
            "SQL must not be built by concatenation"
        );
        for needle in [
            "Bio-LNG",
            "UCOME B100",
            "BIO_LNG",
            "UCOME_B100",
            "8f6f34a5-2927-54a6-8564-cca40ae7cb8a",
            "e561e43f-d9b2-598e-981c-f1d28d515ddc",
        ] {
            assert!(!sql.contains(needle), "{needle} must not appear in SQL");
        }
    }
    // The order statement takes the captured instants; the trade statement
    // takes only the optional confirmation window.
    assert!(public_order_sql().contains("$2::timestamptz"));
    assert!(public_order_sql().contains("$3::text"));
    assert!(trade_evidence_sql().contains("$2::timestamptz"));
}

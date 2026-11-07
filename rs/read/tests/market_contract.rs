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

const PRIVATE_VECTOR: &str = r####"{"note":"synthetic reviewed terms with a private ASK Bio-LNG declaration","row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"9066e752d0f5f8d7a4e380467d51d2f646a68421c6665aa52074f71146a7dd41","lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}}"####;
const NON_ASCII_VECTOR: &str = r####"{"note":"non-ASCII and escaping; different JSON encoders must hash the same bytes","row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":null,"creation_method":"LEGACY_UNKNOWN","support_authorization_id":null,"provenance":"DEMO","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"2027-Q3","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["\u00dcn\u00efc\u00f4d\u00e9"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"0.00","carbon_intensity_method":"cradle-to-grave","energy_density_mj_kg":"49.10","feedstock":"\u65e5\u672c\u8a9e \"in quotes\" \\ backslash\ttab","origin":"\u00dcn\u00efc\u00f6d\u00e9 \ud83c\udf31","off_spec":true,"off_spec_notes":"line one\nline two \u2014 em dash","is_verdaxis_verified":false,"fame_terms":null,"lng_terms":null},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":null,"creation_method":"LEGACY_UNKNOWN","support_authorization_id":null,"provenance":"DEMO","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"2027-Q3","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["\u00dcn\u00efc\u00f4d\u00e9"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"0","carbon_intensity_method":"cradle-to-grave","energy_density_mj_kg":"49.1","feedstock":"\u65e5\u672c\u8a9e \"in quotes\" \\ backslash\ttab","origin":"\u00dcn\u00efc\u00f6d\u00e9 \ud83c\udf31","off_spec":true,"off_spec_notes":"line one\nline two \u2014 em dash","is_verdaxis_verified":false,"fame_terms":null},"digest":"d00d8bc422c929f04d4251334c823ea18a6261530265e7abae92a577972ce29d"}"####;
const FAME_VECTOR: &str = r####"{"note":"synthetic reviewed terms with a private FAME ASK declaration","row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"e561e43f-d9b2-598e-981c-f1d28d515ddc","delivery_point_id":"73835e92-820e-584b-8280-bb61c63aa28e","port_id":"SGSIN","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"lng_terms":null},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"e561e43f-d9b2-598e-981c-f1d28d515ddc","delivery_point_id":"73835e92-820e-584b-8280-bb61c63aa28e","port_id":"SGSIN","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":{"astm_grade":null,"batch_reference":"BATCH-9","certificate_holder":"Jurong Renewables Pte Ltd","certificate_reference":"CERT-0001","certificate_scope":null,"certificate_valid_until":"2027-06-30","cfpp_c":"-5","ci_basis":"ACTUAL","ci_boundary":"WELL_TO_WAKE","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","cloud_point_c":"-3","document_references":["DOC-1"],"en_climate_class":"B","evidence_due":"BEFORE_LOADING","evidence_status":"AVAILABLE","feedstock_origin":"Malaysia","lhv_mj_kg":"37.20","neat_fame":true,"nomination_status":"IDENTIFIED","producing_site":"Jurong Plant","production_origin":"Singapore","quality_evidence":{"batch_reference":"BATCH-9","laboratory":"SGS","reference":"Q-1","results":[{"method":"EN ISO 12937","property":"WATER_MG_KG","value":"120"}],"sampled_on":"2026-09-01","status":"AVAILABLE","tested_on":"2026-09-03"},"schema_version":1,"shipping_location":"Jurong","side":"ASK","standard":"EN_14214","standard_edition":"EN 14214:2019","sustainability_evidence":{"document_type":"POS","due":"BEFORE_LOADING","issuer":"ISCC","quantity_mt":"500.00","reference":"S-1","status":"AVAILABLE","supply_date":"2026-10-01"},"sustainability_scheme":"ISCC_EU","uco_mass_pct":100}},"digest":"e43de0af597fd8ceebe05ac19356b384d97ad3db7728a8b94ec1199f877c987a","fame_terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"}}"####;

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
fn exact_stored_typed_decimals_match_python() {
    let cases = vector(
        r####"[{"name":"fame_v1-FameAskTerms-cfpp_c-1E-29","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"1E-29","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"1E-29","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cfpp_c-0E-100","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"0E-100","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"0E-100","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cfpp_c--0.000","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-0.000","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-0.000","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cfpp_c-1.000000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"1.000000000000000000000000000001","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"1.000000000000000000000000000001","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cloud_point_c-1E-29","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"1E-29","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"1E-29","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cloud_point_c-0E-100","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"0E-100","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"0E-100","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cloud_point_c--0.000","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-0.000","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-0.000","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameAskTerms-cloud_point_c-1.000000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"1.000000000000000000000000000001","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"1.000000000000000000000000000001","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cfpp_c-1E-29","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"1E-29","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"1E-29","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cfpp_c-0E-100","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"0E-100","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"0E-100","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cfpp_c--0.000","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"-0.000","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"-0.000","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cfpp_c-1.000000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"1.000000000000000000000000000001","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":"1.000000000000000000000000000001","max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cloud_point_c-1E-29","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"1E-29","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"1E-29","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cloud_point_c-0E-100","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"0E-100","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"0E-100","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cloud_point_c--0.000","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"-0.000","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"-0.000","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_cloud_point_c-1.000000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"1.000000000000000000000000000001","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":"1.000000000000000000000000000001","max_ci_gco2e_mj":null,"ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_ci_gco2e_mj-1E-29","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"1E-29","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"1E-29","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_ci_gco2e_mj-0E-100","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"0E-100","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"0E-100","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_ci_gco2e_mj--0.000","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"-0.000","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"-0.000","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-FameBidTerms-max_ci_gco2e_mj-1.000000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"1.000000000000000000000000000001","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":"1.000000000000000000000000000001","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"lng_v1-LngAskTerms-min_methane_number-1E-29","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"1E-29","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"1E-29","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-LngAskTerms-min_methane_number-0E-100","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"0E-100","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"0E-100","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-LngAskTerms-min_methane_number--0.000","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"-0.000","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"-0.000","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-LngAskTerms-min_methane_number-1.000000000000000000000000000001","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"1.000000000000000000000000000001","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"1.000000000000000000000000000001","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-LngBidTerms-max_wtt_ghg_gco2e_mj-1E-29","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"1E-29","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"1E-29","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true}},{"name":"lng_v1-LngBidTerms-max_wtt_ghg_gco2e_mj-0E-100","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"0E-100","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"0E-100","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true}},{"name":"lng_v1-LngBidTerms-max_wtt_ghg_gco2e_mj--0.000","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"-0.000","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"-0.000","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true}},{"name":"lng_v1-LngBidTerms-max_wtt_ghg_gco2e_mj-1.000000000000000000000000000001","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"1.000000000000000000000000000001","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":"1.000000000000000000000000000001","accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true}},{"name":"lng_v1-LngBidTerms-min_methane_number-1E-29","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"1E-29","accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"1E-29","accept_resellers":true}},{"name":"lng_v1-LngBidTerms-min_methane_number-0E-100","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"0E-100","accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"0E-100","accept_resellers":true}},{"name":"lng_v1-LngBidTerms-min_methane_number--0.000","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"-0.000","accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"-0.000","accept_resellers":true}},{"name":"lng_v1-LngBidTerms-min_methane_number-1.000000000000000000000000000001","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"1.000000000000000000000000000001","accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":"1.000000000000000000000000000001","accept_resellers":true}},{"name":"quality-1E-29","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"1E-29","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"1E-29","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"quality-0E-100","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"0E-100","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"0E-100","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"quality--0E-100","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"-0E-100","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"-0E-100","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"quality-1000000.0000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"1000000.0000000000000000000000000001","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"accepted":false},{"name":"quality--1E-29","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"-1E-29","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"accepted":false},{"name":"lng_v1-bio_share_pct-1.2300","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"1.2300","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"1.2300","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-bio_share_pct-1.234","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"1.234","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":false},{"name":"lng_v1-bio_share_pct-1E-29","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"1E-29","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":false},{"name":"lng_v1-bio_share_pct-1.230000000000000000000000000001","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"1.230000000000000000000000000001","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"1.230000000000000000000000000001","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-wtt_ghg_gco2e_mj-1.2300","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"1.2300","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"1.2300","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"lng_v1-wtt_ghg_gco2e_mj-1.234","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"1.234","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":false},{"name":"lng_v1-wtt_ghg_gco2e_mj-1E-29","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"1E-29","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":false},{"name":"lng_v1-wtt_ghg_gco2e_mj-1.230000000000000000000000000001","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"1.230000000000000000000000000001","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true,"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"1.230000000000000000000000000001","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"fame_v1-ci_gco2e_mj-1.2300","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"1.2300","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"1.2300","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-ci_gco2e_mj-1.234","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"1.234","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"accepted":false},{"name":"fame_v1-ci_gco2e_mj-1E-29","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"1E-29","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"accepted":false},{"name":"fame_v1-ci_gco2e_mj-1.230000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"1.230000000000000000000000000001","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"1.230000000000000000000000000001","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-lhv_mj_kg-1.2300","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"1.2300","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"1.2300","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"fame_v1-lhv_mj_kg-1.234","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"1.234","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"accepted":false},{"name":"fame_v1-lhv_mj_kg-1E-29","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"1E-29","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"accepted":false},{"name":"fame_v1-lhv_mj_kg-1.230000000000000000000000000001","profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"1.230000000000000000000000000001","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true,"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"1.230000000000000000000000000001","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"min_bio_share_pct-1.000000000000000000000000000001","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":"1.000000000000000000000000000001","max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true},"accepted":true,"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":"1.000000000000000000000000000001","max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true}},{"name":"min_bio_share_pct-0.999999999999999999999999999999","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_bio_share_pct":"0.999999999999999999999999999999"},"accepted":false}]"####,
    );
    for case in cases.as_array().unwrap() {
        let terms = &case["terms"];
        let side = terms["side"].as_str().unwrap();
        let profile = case["profile"].as_str().unwrap();
        let validation = if profile == "lng_v1" {
            validate_lng_terms(side, Some(terms))
        } else {
            validate_fame_terms(side, Some(terms), today())
        };
        assert_eq!(
            validation.is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}: {validation:?}",
            case["name"]
        );
        if case["accepted"] == true {
            assert_eq!(
                redact_terms(profile, terms).unwrap(),
                case["public"],
                "{}",
                case["name"]
            );
        }
    }
}

#[test]
fn trailing_zero_declarations_match_python() {
    let cases = vector(
        r####"[{"profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.500","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true},{"profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"-0.000","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"accepted":true},{"profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.4000","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.20","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true},{"profile":"fame_v1","terms":{"schema_version":1,"neat_fame":true,"batch_reference":"BATCH-9","producing_site":"Jurong Plant","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"lhv_mj_kg":"37.2000","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"nomination_status":"IDENTIFIED","side":"ASK","evidence_due":"BEFORE_LOADING"},"accepted":true}]"####,
    );
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
fn public_asks_ignore_private_validation_but_execution_does_not() {
    for (profile, mut terms, private_fields) in [
        (
            "fame_v1",
            vector(FAME_VECTOR)["fame_terms"].clone(),
            vec![
                "certificate_holder",
                "certificate_reference",
                "batch_reference",
                "producing_site",
                "certificate_scope",
                "document_references",
            ],
        ),
        (
            "lng_v1",
            vector(PRIVATE_VECTOR)["lng_terms"].clone(),
            vec!["scheme_certificate_number", "physical_supplier_name"],
        ),
    ] {
        let expected =
            verdaxis_read::market::terms::public_response_terms(profile, &terms).unwrap();
        for field in private_fields {
            terms[field] = json!("");
            assert_eq!(
                verdaxis_read::market::terms::public_response_terms(profile, &terms).unwrap(),
                expected,
                "{field}"
            );
            let qualified = if profile == "fame_v1" {
                validate_fame_terms("ASK", Some(&terms), today())
            } else {
                validate_lng_terms("ASK", Some(&terms))
            };
            assert!(
                qualified.is_err(),
                "{field} must still fail execution validation"
            );
        }
    }
}

#[test]
fn public_fame_evidence_ignores_private_traceability() {
    use verdaxis_read::market::terms::public_response_terms;
    let original = vector(FAME_VECTOR)["fame_terms"].clone();
    let expected = public_response_terms("fame_v1", &original).unwrap();
    for (section, field) in [
        ("quality_evidence", "reference"),
        ("quality_evidence", "batch_reference"),
        ("quality_evidence", "laboratory"),
        ("quality_evidence", "sampled_on"),
        ("quality_evidence", "tested_on"),
        ("sustainability_evidence", "reference"),
        ("sustainability_evidence", "quantity_mt"),
        ("sustainability_evidence", "supply_date"),
    ] {
        let mut terms = original.clone();
        terms[section][field] = json!({"malformed": true});
        assert_eq!(public_response_terms("fame_v1", &terms).unwrap(), expected);
        assert!(validate_fame_terms("ASK", Some(&terms), today()).is_err());
    }
}

#[test]
fn public_response_still_requires_public_fields() {
    use verdaxis_read::market::terms::public_response_terms;
    for (profile, original, fields) in [
        (
            "fame_v1",
            vector(FAME_VECTOR)["fame_terms"].clone(),
            vec![
                "nomination_status",
                "standard",
                "certificate_valid_until",
                "evidence_due",
            ],
        ),
        (
            "lng_v1",
            vector(PRIVATE_VECTOR)["lng_terms"].clone(),
            vec![
                "production_country",
                "bio_share_pct",
                "seller_is_physical_supplier",
            ],
        ),
    ] {
        for field in fields {
            let mut terms = original.clone();
            terms.as_object_mut().unwrap().remove(field);
            assert!(public_response_terms(profile, &terms).is_err(), "{field}");
        }
    }
    // `FameQualitySummary` requires `results`, but the public response is
    // always built from the full declaration's dump, where an omitted list
    // already became []. Python accepts it and answers results: [].
    let mut fame = vector(FAME_VECTOR)["fame_terms"].clone();
    fame["quality_evidence"]
        .as_object_mut()
        .unwrap()
        .remove("results");
    assert_eq!(
        public_response_terms("fame_v1", &fame).unwrap()["quality_evidence"]["results"],
        json!([])
    );
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
    let cases = vector(
        r####"[{"name":"quality-120.00","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120.00","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120.00","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"quality--0.00","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"-0.00","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"-0.00","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"quality-0.00000001","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"0.00000001","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"1E-8","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"lng_v1-BID","profile":"lng_v1","terms":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_delivery_methods":["SHIP_TO_SHIP"]},"public":{"side":"BID","accepted_schemes":["ISCC_EU"],"accepted_custody":["MASS_BALANCE","PHYSICAL_SEGREGATION"],"require_union_database":true,"min_bio_share_pct":null,"max_wtt_ghg_gco2e_mj":null,"accepted_delivery_methods":["SHIP_TO_SHIP"],"min_methane_number":null,"accept_resellers":true}},{"name":"fame_v1-BID","profile":"fame_v1","terms":{"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","sustainability_scheme":"ISCC_EU"},"public":{"schema_version":1,"side":"BID","neat_fame":true,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":null,"max_cfpp_c":null,"max_cloud_point_c":null,"max_ci_gco2e_mj":null,"ci_methodology":null,"ci_boundary":null,"ci_basis":null,"sustainability_scheme":"ISCC_EU","require_quality_evidence":false,"require_sustainability_evidence":false,"evidence_due":"BEFORE_LOADING"}},{"name":"lng_v1-ASK","profile":"lng_v1","terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"},"public":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","reseller":true,"fueleu_recognition":"UNION_DATABASE"}},{"name":"fame_v1-ASK","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"AVAILABLE","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"pending-quality-defaults","profile":"fame_v1","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"PENDING"},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"public":{"schema_version":1,"neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_valid_until":"2027-06-30","evidence_status":"AVAILABLE","quality_evidence":{"status":"PENDING","results":[]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","due":"BEFORE_LOADING"},"side":"ASK","evidence_due":"BEFORE_LOADING"}},{"name":"lng_v1-null","profile":"lng_v1","terms":null,"public":null},{"name":"fame_v1-null","profile":"fame_v1","terms":null,"public":null}]"####,
    );
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
    let cases = vector(
        r####"[{"row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"100000000000000000000000000000","carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"100000000000000000000000000000","carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"5bcbf1889845cecc60f33d7d22edf8dbe7540bb09b2f28692ffe49da87a86d8c"},{"row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"12345678901234567890123456789","carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"12345678901234567890123456790","carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"4c2bd0f4c91e50f6836c071620831384810e9571f9cca8a8480f1ce7ae28b936"},{"row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"12345678901234567890123456785","carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"12345678901234567890123456780","carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"2c44ac1613d75d7b4b3c389935a372e4bba41341d0ba1fc6b40cd1b5c6578d26"},{"row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"0.00000000000000000000000000001","carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"0.00000000000000000000000000001","carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"f8a0a92d88f1f645912d65e03b3a8a2785c9904f4c9fcc2b13714277d20863d3"},{"row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"-0.00","carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":"0","carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"c5f8412177a44dc2c4f3cfc556ffff77597f82f2acd3226b13b471af0b99ab98"}]"####,
    );
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
    let cases = vector(
        r####"[{"input":"\uff12\uff10\uff12\uff16-Q1","normalized":"\uff12\uff10\uff12\uff16-Q1","start":"2026-01-01","label":"Q1 \uff12\uff10\uff12\uff16"},{"input":"0000-Q1","normalized":"0000-Q1","start":null,"label":"Q1 0000"},{"input":"0026-Q1","normalized":"0026-Q1","start":"0026-01-01","label":"Q1 0026"},{"input":"0026-CAL","normalized":"0026-CAL","start":"0026-01-01","label":"CAL 0026"},{"input":"\u0662\u0660\u0662\u0666-01","normalized":"\u0662\u0660\u0662\u0666-01","start":"2026-01-01","label":"Jan 2026"}]"####,
    );
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
    let cases = vector(
        r####"[{"instant":"2026-10-15T15:59:59+00:00","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2026-10-15","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-10-15","tested_on":"2026-10-15","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"qualified":true},{"instant":"2026-10-15T15:59:59+00:00","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2026-10-15","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-10-16","tested_on":"2026-10-16","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"qualified":false},{"instant":"2026-10-15T16:00:00+00:00","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2026-10-15","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-10-15","tested_on":"2026-10-15","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"qualified":false},{"instant":"2026-10-15T16:00:00+00:00","terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2026-10-15","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-10-16","tested_on":"2026-10-16","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"qualified":false}]"####,
    );
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
        specification_standard: None,
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
        specification_standard: None,
        carbon_intensity_method: None,
        carbon_intensity_gco2_mj: None,
        feedstock: None,
        origin: None,
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
        specification_standard: None,
        carbon_intensity_method: None,
        carbon_intensity_gco2_mj: None,
        feedstock: None,
        origin: None,
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
        specification_standard: None,
        carbon_intensity_method: None,
        carbon_intensity_gco2_mj: None,
        feedstock: None,
        origin: None,
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
// Python schema defaults (app/schemas/fame.py, app/schemas/fame_order.py).
// ---------------------------------------------------------------------------

#[test]
fn fame_ask_document_references_default_to_an_empty_list() {
    // `document_references` is declared with `default_factory=list`, so a
    // declaration may omit the key and the stored projection carries [].
    let mut terms = valid_fame_ask();
    terms.as_object_mut().unwrap().remove("document_references");
    assert!(validate_fame_terms("ASK", Some(&terms), today()).is_ok());
    let owner = verdaxis_read::market::terms::owner_response_terms("fame_v1", &terms).unwrap();
    assert_eq!(owner["document_references"], json!([]));

    // An explicit null is not the default; the field is not Optional.
    let mut null_documents = valid_fame_ask();
    null_documents["document_references"] = json!(null);
    assert!(validate_fame_terms("ASK", Some(&null_documents), today()).is_err());

    // The AVAILABLE rule still refuses the defaulted empty list, with the
    // schema validator's own message.
    let mut available = terms.clone();
    available["evidence_status"] = json!("AVAILABLE");
    assert_eq!(
        validate_fame_terms("ASK", Some(&available), today()),
        Err("document_references are required when evidence is AVAILABLE".to_owned())
    );
}

#[test]
fn fame_quality_results_default_to_an_empty_list() {
    // FameQualityEvidence defaults `results` to [], so a pending record may
    // omit it and both projections carry the empty list.
    let mut terms = valid_fame_ask();
    terms["quality_evidence"] = json!({"status": "PENDING"});
    assert!(validate_fame_terms("ASK", Some(&terms), today()).is_ok());
    let owner = verdaxis_read::market::terms::owner_response_terms("fame_v1", &terms).unwrap();
    assert_eq!(owner["quality_evidence"]["results"], json!([]));
    let public = verdaxis_read::market::terms::public_response_terms("fame_v1", &terms).unwrap();
    assert_eq!(public["quality_evidence"]["results"], json!([]));

    // An explicit null is not the default; the field is not Optional.
    let mut null_results = valid_fame_ask();
    null_results["quality_evidence"] = json!({"status": "PENDING", "results": null});
    assert!(validate_fame_terms("ASK", Some(&null_results), today()).is_err());
}

#[test]
fn fame_literal_defaults_reject_an_explicit_null() {
    // `schema_version: Literal[1] = 1` and
    // `nomination_status: Literal["PENDING", "IDENTIFIED"] = "PENDING"` are
    // not Optional: the default applies only when the key is absent.
    for field in ["schema_version", "nomination_status"] {
        let mut ask = valid_fame_ask();
        ask[field] = json!(null);
        assert!(
            validate_fame_terms("ASK", Some(&ask), today()).is_err(),
            "{field}: an explicit null must be rejected"
        );
        let mut bid = json!({
            "side": "BID",
            "neat_fame": true,
            "standard": "EN_14214",
            "standard_edition": "EN 14214:2019",
            "sustainability_scheme": "ISCC_EU"
        });
        bid[field] = json!(null);
        assert!(
            validate_fame_terms("BID", Some(&bid), today()).is_err(),
            "{field}: an explicit null must be rejected on a BID"
        );
    }

    // An omitted nomination_status still defaults to PENDING in storage.
    let mut pending = valid_fame_ask();
    pending.as_object_mut().unwrap().remove("nomination_status");
    assert!(validate_fame_terms("ASK", Some(&pending), today()).is_ok());
    let owner = verdaxis_read::market::terms::owner_response_terms("fame_v1", &pending).unwrap();
    assert_eq!(owner["nomination_status"], json!("PENDING"));
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

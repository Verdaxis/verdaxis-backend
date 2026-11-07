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
        "fame_v1" => {
            r####"{"note":"synthetic reviewed terms with a private FAME ASK declaration","row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"e561e43f-d9b2-598e-981c-f1d28d515ddc","delivery_point_id":"73835e92-820e-584b-8280-bb61c63aa28e","port_id":"SGSIN","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"},"lng_terms":null},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"e561e43f-d9b2-598e-981c-f1d28d515ddc","delivery_point_id":"73835e92-820e-584b-8280-bb61c63aa28e","port_id":"SGSIN","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":{"astm_grade":null,"batch_reference":"BATCH-9","certificate_holder":"Jurong Renewables Pte Ltd","certificate_reference":"CERT-0001","certificate_scope":null,"certificate_valid_until":"2027-06-30","cfpp_c":"-5","ci_basis":"ACTUAL","ci_boundary":"WELL_TO_WAKE","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","cloud_point_c":"-3","document_references":["DOC-1"],"en_climate_class":"B","evidence_due":"BEFORE_LOADING","evidence_status":"AVAILABLE","feedstock_origin":"Malaysia","lhv_mj_kg":"37.20","neat_fame":true,"nomination_status":"IDENTIFIED","producing_site":"Jurong Plant","production_origin":"Singapore","quality_evidence":{"batch_reference":"BATCH-9","laboratory":"SGS","reference":"Q-1","results":[{"method":"EN ISO 12937","property":"WATER_MG_KG","value":"120"}],"sampled_on":"2026-09-01","status":"AVAILABLE","tested_on":"2026-09-03"},"schema_version":1,"shipping_location":"Jurong","side":"ASK","standard":"EN_14214","standard_edition":"EN 14214:2019","sustainability_evidence":{"document_type":"POS","due":"BEFORE_LOADING","issuer":"ISCC","quantity_mt":"500.00","reference":"S-1","status":"AVAILABLE","supply_date":"2026-10-01"},"sustainability_scheme":"ISCC_EU","uco_mass_pct":100}},"digest":"e43de0af597fd8ceebe05ac19356b384d97ad3db7728a8b94ec1199f877c987a","fame_terms":{"schema_version":1,"side":"ASK","neat_fame":true,"nomination_status":"IDENTIFIED","uco_mass_pct":100,"standard":"EN_14214","standard_edition":"EN 14214:2019","astm_grade":null,"en_climate_class":"B","production_origin":"Singapore","feedstock_origin":"Malaysia","shipping_location":"Jurong","cfpp_c":"-5","cloud_point_c":"-3","ci_gco2e_mj":"18.40","ci_methodology":"ISCC","ci_boundary":"WELL_TO_WAKE","ci_basis":"ACTUAL","lhv_mj_kg":"37.20","sustainability_scheme":"ISCC_EU","certificate_reference":"CERT-0001","certificate_holder":"Jurong Renewables Pte Ltd","certificate_valid_until":"2027-06-30","certificate_scope":null,"evidence_status":"AVAILABLE","document_references":["DOC-1"],"batch_reference":"BATCH-9","producing_site":"Jurong Plant","quality_evidence":{"status":"AVAILABLE","reference":"Q-1","batch_reference":"BATCH-9","laboratory":"SGS","sampled_on":"2026-09-01","tested_on":"2026-09-03","results":[{"property":"WATER_MG_KG","value":"120","method":"EN ISO 12937"}]},"sustainability_evidence":{"status":"AVAILABLE","document_type":"POS","reference":"S-1","issuer":"ISCC","quantity_mt":"500.00","supply_date":"2026-10-01","due":"BEFORE_LOADING"},"evidence_due":"BEFORE_LOADING"}}"####
        }
        _ => {
            r####"{"note":"synthetic reviewed terms with a private ASK Bio-LNG declaration","row":{"id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500.0000","remaining_quantity_mt":"125.5000","price_per_mt_usd":"1180.50","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456+00:00","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.10","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}},"canonical_terms":{"order_id":"8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","side":"ASK","organization_id":"0f0e0d0c-0b0a-4908-8706-050403020100","owner_user_id":null,"created_by_actor_user_id":"11111111-2222-4333-8444-555555555555","creation_method":"MARKET_SUPPORT","support_authorization_id":"99999999-8888-4777-8666-555544443333","provenance":"REAL","product_id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","delivery_point_id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","port_id":"NLRTM","vessel_id":null,"inventory_item_id":null,"quantity_mt":"500","remaining_quantity_mt":"125.5","price_per_mt_usd":"1180.5","availability_window":"SPOT","delivery_window_start":"2026-11-01","delivery_window_end":null,"expires_at":"2026-10-15T12:00:00.123456Z","certifications":["ISCC EU","REDcert-EU"],"certification_declared":true,"certification_scheme":"ISCC EU","specification_standard":"ISO 23306:2020","msds_available":true,"carbon_intensity_gco2_mj":null,"carbon_intensity_method":null,"energy_density_mj_kg":"49.1","feedstock":"Manure","origin":"NL","off_spec":false,"off_spec_notes":null,"is_verdaxis_verified":false,"fame_terms":null,"lng_terms":{"bio_share_pct":"95.50","chain_of_custody":"MASS_BALANCE","delivery_method":"SHIP_TO_SHIP","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","min_methane_number":"80","physical_supplier_name":"Rotterdam Biofuels BV","pos_timing":"WITH_DELIVERY","production_country":"NL","scheme_certificate_number":"ISCC-EU-0001","seller_is_physical_supplier":false,"side":"ASK","specification_standard":"ISO 23306:2020","sustainability_scheme":"ISCC_EU","union_database_registered":true,"wtt_ghg_gco2e_mj":"12.30"}},"digest":"9066e752d0f5f8d7a4e380467d51d2f646a68421c6665aa52074f71146a7dd41","lng_terms":{"side":"ASK","sustainability_scheme":"ISCC_EU","chain_of_custody":"MASS_BALANCE","union_database_registered":true,"bio_share_pct":"95.50","feedstock":"MANURE","feedstock_detail":"Slurry from dairy herds","production_country":"NL","wtt_ghg_gco2e_mj":"12.30","specification_standard":"ISO 23306:2020","min_methane_number":"80","delivery_method":"SHIP_TO_SHIP","seller_is_physical_supplier":false,"pos_timing":"WITH_DELIVERY","scheme_certificate_number":"ISCC-EU-0001","physical_supplier_name":"Rotterdam Biofuels BV"}}"####
        }
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
        .find(|product| {
            product.qualification_profile == "scalar" && product.specification_standard.is_none()
        })
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

#[test]
fn python_control_whitespace_does_not_qualify_scalar_asks() {
    let policy = policy();
    let product = policy
        .products
        .iter()
        .find(|p| p.qualification_profile == "scalar" && p.specification_standard.is_none())
        .unwrap();
    let mut row = typed_row("fame_v1");
    row["product_id"] = json!(product.id);
    row["carbon_intensity_gco2_mj"] = json!("0.00");
    row["origin"] = json!("valid origin");
    row["certification_scheme"] = json!("  custom scheme  ");
    assert!(candidate_is_benchmark_qualified(&policy, &row, &clock()));
    for character in ['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}'] {
        for field in [
            "origin",
            "specification_standard",
            "feedstock",
            "certification_scheme",
        ] {
            let mut invalid = row.clone();
            invalid[field] = json!(character.to_string());
            assert!(
                !candidate_is_benchmark_qualified(&policy, &invalid, &clock()),
                "{field}: {character:?}"
            );
        }
    }
}

#[test]
fn legal_cent_prices_round_weighted_half_cents() {
    for (left, right, expected) in [
        ("100.00", "100.01", "100.00"),
        ("100.01", "100.02", "100.02"),
    ] {
        assert_eq!(
            qualified_vwap(&[
                (D::TEN, D::from_str(left).unwrap()),
                (D::TEN, D::from_str(right).unwrap())
            ])
            .unwrap()
            .to_string(),
            expected
        );
    }
    let rows = vec![(D::from(100_000), D::from(1_000_000)); 1000];
    assert_eq!(qualified_vwap(&rows).unwrap().to_string(), "1000000.00");
}

#[test]
fn whitespace_matches_python_unicode_inventory() {
    let expected = [
        9, 10, 11, 12, 13, 28, 29, 30, 31, 32, 133, 160, 5760, 8192, 8193, 8194, 8195, 8196, 8197,
        8198, 8199, 8200, 8201, 8202, 8232, 8233, 8239, 8287, 12288,
    ];
    let actual: Vec<_> = (0..=0x10ffff)
        .filter_map(char::from_u32)
        .filter(|c| verdaxis_read::market::whitespace::is_python_whitespace(*c))
        .map(u32::from)
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn excluded_origin_row_changes_price_quantity_and_count() {
    let policy = policy();
    let product = policy
        .products
        .iter()
        .find(|p| p.qualification_profile == "scalar" && p.specification_standard.is_none())
        .unwrap();
    let mut row = typed_row("fame_v1");
    row["product_id"] = json!(product.id);
    row["carbon_intensity_gco2_mj"] = json!("0.00");
    row["origin"] = json!("valid");
    let mut invalid = row.clone();
    invalid["origin"] = json!("\u{001c}");
    let rows: Vec<_> = [(row, D::from(100)), (invalid, D::from(200))]
        .into_iter()
        .filter(|(row, _)| candidate_is_benchmark_qualified(&policy, row, &clock()))
        .map(|(_, price)| (D::TEN, price))
        .collect();
    assert_eq!(qualified_vwap(&rows).unwrap().to_string(), "100.00");
    assert_eq!(rows.iter().map(|(q, _)| *q).sum::<D>(), D::TEN);
    assert_eq!(rows.len(), 1);
}

#[test]
fn wrong_lane_lng_is_not_a_benchmark() {
    let policy = policy();
    let mut row = typed_row("lng_v1");
    let product = policy
        .products
        .iter()
        .find(|p| p.qualification_profile == "lng_v1")
        .unwrap();
    let point = policy
        .points
        .iter()
        .find(|p| !product.lanes.contains(&p.id))
        .unwrap();
    row["delivery_point_id"] = json!(point.id);
    assert!(!candidate_is_benchmark_qualified(&policy, &row, &clock()));
}

#[test]
fn fixed_biofuel_asks_need_the_policy_standard_and_full_metadata() {
    let policy = policy();
    let product = policy
        .products
        .iter()
        .find(|p| p.specification_standard.is_some())
        .unwrap();
    let mut row = typed_row("fame_v1");
    row["product_id"] = json!(product.id);
    row["carbon_intensity_gco2_mj"] = json!("79.90");
    row["origin"] = json!("Singapore");
    row["carbon_intensity_method"] = json!("Whole-blend CI and LCV");
    // The policy standard is required verbatim (after trim/case).
    row["specification_standard"] = json!(product.specification_standard.clone());
    assert!(candidate_is_benchmark_qualified(&policy, &row, &clock()));
    let mut wrong = row.clone();
    wrong["specification_standard"] = json!("EN 14214");
    assert!(!candidate_is_benchmark_qualified(&policy, &wrong, &clock()));
    // Trim and case fold match Python's upper(trim(...)) comparison.
    let mut padded = row.clone();
    padded["specification_standard"] =
        json!(format!(" {} ", product.specification_standard.clone().unwrap()).to_lowercase());
    assert!(candidate_is_benchmark_qualified(&policy, &padded, &clock()));
    // Every metadata arm is required.
    for field in ["carbon_intensity_method", "feedstock", "origin"] {
        let mut invalid = row.clone();
        invalid[field] = json!(" \t\n");
        assert!(
            !candidate_is_benchmark_qualified(&policy, &invalid, &clock()),
            "{field}"
        );
    }
    let mut no_ci = row.clone();
    no_ci["carbon_intensity_gco2_mj"] = Value::Null;
    assert!(!candidate_is_benchmark_qualified(&policy, &no_ci, &clock()));
    // A BID never needs the supplier metadata.
    let mut bid = row;
    bid["side"] = json!("BID");
    bid["certification_scheme"] = Value::Null;
    bid["certification_declared"] = json!(false);
    assert!(candidate_is_benchmark_qualified(&policy, &bid, &clock()));
}

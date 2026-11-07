//! Task 5: the market catalog policy is attested data, never code.
//!
//! These tests load the committed policy artifact, prove strict identity
//! matching and the captured catalog projections, and scan executable Rust and
//! SQL for product names, codes, and UUIDs that must live only in data.

use std::path::{Path, PathBuf};

use serde_json::json;
use verdaxis_read::market::policy::{load_policy, point_matches, product_matches, project_catalog};
use verdaxis_read::{Json, MarketPolicy, PointPolicy, ProductPolicy};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const POLICY_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/market-policy.json"
));
const MCP_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/mcp-catalog.json"
));

fn fixture_product(id: u128, code: &str, name: &str, order: u32) -> ProductPolicy {
    ProductPolicy {
        id: uuid::Uuid::from_u128(id),
        code: code.into(),
        name: name.into(),
        fuel_type: "Fixture type".into(),
        fuel_grade: "Fixture grade".into(),
        execution_mode: "ORDERBOOK".into(),
        lanes: vec![uuid::Uuid::from_u128(0x1000)],
        qualification_profile: "scalar".into(),
        specification_standard: None,
        display_order: order,
    }
}

fn fixture_point() -> PointPolicy {
    PointPolicy {
        id: uuid::Uuid::from_u128(0x1000),
        name: "Fixture point".into(),
        region: "Fixture region".into(),
        timezone: "UTC".into(),
        display_order: 1,
    }
}

/// Three separate identities. The founder requires B30, B100, and UCOME_B100 to
/// stay distinct, so nothing may merge or rename them.
fn fixture_policy() -> MarketPolicy {
    MarketPolicy {
        source_sha: "a".repeat(40),
        products: vec![
            fixture_product(0xb30, "FIXTURE_B30", "Fixture B30 blend", 1),
            fixture_product(0xb100, "FIXTURE_B100", "Fixture B100 neat", 2),
            fixture_product(0x0c0c, "FIXTURE_UCOME", "Fixture UCOME B100", 3),
        ],
        points: vec![fixture_point()],
    }
}

#[test]
fn active_alias_is_not_a_canonical_product() {
    let p = ProductPolicy {
        id: uuid::Uuid::nil(),
        code: "TEST_CODE".into(),
        name: "Fixture product".into(),
        fuel_type: "Fixture type".into(),
        fuel_grade: "Fixture grade".into(),
        execution_mode: "ORDERBOOK".into(),
        lanes: vec![],
        qualification_profile: "scalar".into(),
        specification_standard: None,
        display_order: 1,
    };
    assert!(product_matches(
        &p,
        p.id,
        &p.name,
        &p.fuel_type,
        &p.fuel_grade,
        true
    ));
    assert!(!product_matches(
        &p,
        p.id,
        "Alias",
        &p.fuel_type,
        &p.fuel_grade,
        true
    ));
    assert!(!product_matches(
        &p,
        p.id,
        &p.name,
        &p.fuel_type,
        &p.fuel_grade,
        false
    ));
}

#[test]
fn three_distinct_alcohol_identities_are_not_merged() {
    let policy = fixture_policy();
    let names: Vec<&str> = policy.products.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Fixture B30 blend",
            "Fixture B100 neat",
            "Fixture UCOME B100"
        ]
    );
    assert_eq!(
        policy
            .products
            .iter()
            .map(|p| p.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for product in &policy.products {
        assert!(product_matches(
            product,
            product.id,
            &product.name,
            &product.fuel_type,
            &product.fuel_grade,
            true
        ));
        for other in &policy.products {
            if other.id != product.id {
                assert!(!product_matches(
                    product,
                    other.id,
                    &other.name,
                    &other.fuel_type,
                    &other.fuel_grade,
                    true
                ));
            }
        }
    }
}

#[test]
fn a_new_policy_record_needs_no_code_change() {
    let mut policy = fixture_policy();
    let extra = fixture_product(0x0e57, "FIXTURE_EXTRA", "Fixture extra", 4);
    policy.products.push(extra.clone());
    assert!(product_matches(
        &extra,
        extra.id,
        &extra.name,
        &extra.fuel_type,
        &extra.fuel_grade,
        true
    ));
    // The original records keep their identity; nothing is remapped.
    assert_eq!(policy.products[0].name, "Fixture B30 blend");
    assert_eq!(policy.products[1].code, "FIXTURE_B100");
    let loaded = load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).unwrap();
    assert_eq!(loaded.products.len(), 4);
}

#[test]
fn policy_artifact_loads_with_its_recorded_source_sha() {
    let raw: Json = serde_json::from_slice(POLICY_BYTES).unwrap();
    let sha = raw["source_sha"].as_str().unwrap().to_owned();
    let policy = load_policy(POLICY_BYTES, &sha).unwrap();
    assert_eq!(policy.source_sha, sha);
    assert_eq!(policy.products.len(), 8);
    assert_eq!(policy.points.len(), 8);
    let profiles: std::collections::BTreeSet<&str> = policy
        .products
        .iter()
        .map(|p| p.qualification_profile.as_str())
        .collect();
    assert!(profiles.is_subset(&std::collections::BTreeSet::from([
        "scalar", "fame_v1", "lng_v1"
    ])));
    for product in &policy.products {
        assert!(!product.lanes.is_empty(), "{} has no lane", product.code);
        assert!(product.execution_mode == "ORDERBOOK");
    }
}

#[test]
fn policy_load_rejects_a_wrong_source_sha() {
    let error = match load_policy(POLICY_BYTES, &"b".repeat(40)) {
        Ok(_) => panic!("a wrong source SHA must be refused"),
        Err(error) => error,
    };
    assert!(error.contains("source"), "unexpected error: {error}");
}

#[test]
fn policy_load_rejects_duplicate_codes_uuids_profiles_lanes_and_points() {
    let mut policy = fixture_policy();
    let duplicate_code = fixture_product(0x9001, "FIXTURE_B30", "Fixture other", 4);
    policy.products.push(duplicate_code);
    assert!(load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).is_err());

    let mut policy = fixture_policy();
    let duplicate_id = fixture_product(0xb30, "FIXTURE_OTHER", "Fixture other", 4);
    policy.products.push(duplicate_id);
    assert!(load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).is_err());

    let mut policy = fixture_policy();
    policy.products[0].qualification_profile = "not_a_rule_version".into();
    assert!(load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).is_err());

    let mut policy = fixture_policy();
    policy.products[0].lanes = vec![];
    assert!(load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).is_err());

    let mut policy = fixture_policy();
    policy.products[0].lanes = vec![uuid::Uuid::from_u128(0xdead)];
    assert!(load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).is_err());

    let mut policy = fixture_policy();
    policy.points.push(fixture_point());
    assert!(load_policy(&serde_json::to_vec(&policy).unwrap(), &policy.source_sha).is_err());
}

#[test]
fn point_matching_is_a_strict_identity_tuple() {
    let point = fixture_point();
    assert!(point_matches(
        &point,
        point.id,
        &point.name,
        &point.region,
        true
    ));
    assert!(!point_matches(
        &point,
        point.id,
        "Alias",
        &point.region,
        true
    ));
    assert!(!point_matches(
        &point,
        point.id,
        &point.name,
        "Elsewhere",
        true
    ));
    assert!(!point_matches(
        &point,
        point.id,
        &point.name,
        &point.region,
        false
    ));
}

fn vector(name: &str) -> Json {
    let text = match name {
        "points_expected.json" => {
            r####"[{"id":"78281fb3-e726-5396-802e-34b01fda21e8","name":"Dalian","region":"Asia","timezone":"Asia/Shanghai","is_active":true},{"id":"262d36ae-6f35-5785-b9e8-9e221b0f1b78","name":"Busan","region":"Asia","timezone":"Asia/Seoul","is_active":true},{"id":"633c0593-f9b9-52ad-abe5-a0285f6888d3","name":"Shanghai","region":"Asia","timezone":"Asia/Shanghai","is_active":true},{"id":"73835e92-820e-584b-8280-bb61c63aa28e","name":"Singapore","region":"Asia","timezone":"Asia/Singapore","is_active":true},{"id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","name":"Rotterdam","region":"Europe","timezone":"Europe/Amsterdam","is_active":true},{"id":"a083db06-b050-56c2-a274-3897eac2fdae","name":"Houston","region":"Americas","timezone":"America/Chicago","is_active":true},{"id":"1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","name":"Los Angeles","region":"Americas","timezone":"America/Los_Angeles","is_active":true}]"####
        }
        "points_rows.json" => {
            r####"[{"id":"78281fb3-e726-5396-802e-34b01fda21e8","name":"Dalian","region":"Asia","timezone":"Asia/Shanghai","is_active":true},{"id":"262d36ae-6f35-5785-b9e8-9e221b0f1b78","name":"Busan","region":"Asia","timezone":"Asia/Seoul","is_active":true},{"id":"633c0593-f9b9-52ad-abe5-a0285f6888d3","name":"Shanghai","region":"Asia","timezone":"Asia/Shanghai","is_active":true},{"id":"73835e92-820e-584b-8280-bb61c63aa28e","name":"Singapore","region":"Asia","timezone":"Asia/Singapore","is_active":true},{"id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","name":"Rotterdam","region":"Europe","timezone":"Europe/Amsterdam","is_active":true},{"id":"a083db06-b050-56c2-a274-3897eac2fdae","name":"Houston","region":"Americas","timezone":"America/Chicago","is_active":true},{"id":"1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","name":"Los Angeles","region":"Americas","timezone":"America/Los_Angeles","is_active":true},{"id":"78281fb3-e726-5396-802e-34b01fda21e8","name":"Alias Point","region":"Asia","timezone":"Asia/Shanghai","is_active":true},{"id":"00000000-0000-4000-8000-0000000000ff","name":"Dalian","region":"Asia","timezone":"Asia/Shanghai","is_active":true},{"id":"1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","name":"Rotterdam","region":"Europe","timezone":"Europe/Amsterdam","is_active":false}]"####
        }
        "products_expected.json" => {
            r####"[{"id":"c4cecebc-3d2d-5840-8021-57a9a11bc673","name":"B30","market_product":"B30","execution_mode":"ORDERBOOK","available_delivery_point_ids":["78281fb3-e726-5396-802e-34b01fda21e8","262d36ae-6f35-5785-b9e8-9e221b0f1b78","633c0593-f9b9-52ad-abe5-a0285f6888d3","73835e92-820e-584b-8280-bb61c63aa28e","1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","a083db06-b050-56c2-a274-3897eac2fdae","1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","a4762444-4e0b-5bc1-b647-a185047adae5"],"spec_description":"30% FAME by volume blended with 70% VLSFO; finished blend meets ISO 8217:2024 RF 380 with sulphur at most 0.50% by mass","fuel_type":"Biofuel","fuel_grade":"B30","unit":"MT","min_lot_size":"200.00","is_active":true},{"id":"74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5","name":"B100","market_product":"B100","execution_mode":"ORDERBOOK","available_delivery_point_ids":["78281fb3-e726-5396-802e-34b01fda21e8","262d36ae-6f35-5785-b9e8-9e221b0f1b78","633c0593-f9b9-52ad-abe5-a0285f6888d3","73835e92-820e-584b-8280-bb61c63aa28e","1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","a083db06-b050-56c2-a274-3897eac2fdae","1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","a4762444-4e0b-5bc1-b647-a185047adae5"],"spec_description":"100% FAME, excluding HVO; finished fuel meets ISO 8217:2024 DFA with sulphur at most 0.10% by mass","fuel_type":"Biofuel","fuel_grade":"B100","unit":"MT","min_lot_size":"200.00","is_active":true},{"id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","name":"Bio-LNG","market_product":"BIO_LNG","execution_mode":"ORDERBOOK","available_delivery_point_ids":["1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"],"spec_description":"Liquefied biomethane (bio-LNG) for marine fuel use, delivered to ISO 23306:2020 with a certified Proof of Sustainability.","fuel_type":"LNG","fuel_grade":"Bio","unit":"MT","min_lot_size":"1.00","is_active":true},{"id":"e561e43f-d9b2-598e-981c-f1d28d515ddc","name":"UCOME B100","market_product":"UCOME_B100","execution_mode":"ORDERBOOK","available_delivery_point_ids":["73835e92-820e-584b-8280-bb61c63aa28e"],"spec_description":"Neat B100 used cooking oil methyl ester for wholesale Singapore trading. 1 MT is the platform input minimum; orders require compatible quality and delivery terms.","fuel_type":"FAME","fuel_grade":"UCOME","unit":"MT","min_lot_size":"1.00","is_active":true},{"id":"9510c713-6e39-5080-add3-0a7c29657b79","name":"Bio Methanol","market_product":"BIO_METHANOL","execution_mode":"ORDERBOOK","available_delivery_point_ids":["78281fb3-e726-5396-802e-34b01fda21e8","262d36ae-6f35-5785-b9e8-9e221b0f1b78","633c0593-f9b9-52ad-abe5-a0285f6888d3","73835e92-820e-584b-8280-bb61c63aa28e","1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","a083db06-b050-56c2-a274-3897eac2fdae","1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","a4762444-4e0b-5bc1-b647-a185047adae5"],"spec_description":"Bio-methanol produced from biogenic feedstocks for marine fuel use","fuel_type":"Methanol","fuel_grade":"Bio","unit":"MT","min_lot_size":"200.00","is_active":true},{"id":"f9b20492-b445-59cd-b292-a386d913f488","name":"e-Methanol","market_product":"E_METHANOL","execution_mode":"ORDERBOOK","available_delivery_point_ids":["78281fb3-e726-5396-802e-34b01fda21e8","262d36ae-6f35-5785-b9e8-9e221b0f1b78","633c0593-f9b9-52ad-abe5-a0285f6888d3","73835e92-820e-584b-8280-bb61c63aa28e","1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","a083db06-b050-56c2-a274-3897eac2fdae","1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","a4762444-4e0b-5bc1-b647-a185047adae5"],"spec_description":"Synthetic methanol produced from renewable hydrogen and captured CO2","fuel_type":"Methanol","fuel_grade":"E","unit":"MT","min_lot_size":"200.00","is_active":true},{"id":"c4a688be-f7c2-5edc-8f93-6b34e387609c","name":"Bio Ethanol","market_product":"BIO_ETHANOL","execution_mode":"ORDERBOOK","available_delivery_point_ids":["78281fb3-e726-5396-802e-34b01fda21e8","262d36ae-6f35-5785-b9e8-9e221b0f1b78","633c0593-f9b9-52ad-abe5-a0285f6888d3","73835e92-820e-584b-8280-bb61c63aa28e","1379d36c-1ca9-55b7-9c0d-5235a0ba1f36","a083db06-b050-56c2-a274-3897eac2fdae","1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534","a4762444-4e0b-5bc1-b647-a185047adae5"],"spec_description":"Second-generation bioethanol from waste feedstocks","fuel_type":"Ethanol","fuel_grade":"Bio","unit":"MT","min_lot_size":"200.00","is_active":true}]"####
        }
        "products_rows.json" => {
            r####"[{"id":"c4cecebc-3d2d-5840-8021-57a9a11bc673","name":"B30","fuel_type":"Biofuel","fuel_grade":"B30","unit":"MT","min_lot_size":"200.00","spec_description":"30% FAME by volume blended with 70% VLSFO; finished blend meets ISO 8217:2024 RF 380 with sulphur at most 0.50% by mass","is_active":true},{"id":"74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5","name":"B100","fuel_type":"Biofuel","fuel_grade":"B100","unit":"MT","min_lot_size":"200.00","spec_description":"100% FAME, excluding HVO; finished fuel meets ISO 8217:2024 DFA with sulphur at most 0.10% by mass","is_active":true},{"id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","name":"Bio-LNG","fuel_type":"LNG","fuel_grade":"Bio","unit":"MT","min_lot_size":"1.00","spec_description":"Liquefied biomethane (bio-LNG) for marine fuel use, delivered to ISO 23306:2020 with a certified Proof of Sustainability.","is_active":true},{"id":"e561e43f-d9b2-598e-981c-f1d28d515ddc","name":"UCOME B100","fuel_type":"FAME","fuel_grade":"UCOME","unit":"MT","min_lot_size":"1.00","spec_description":"Neat B100 used cooking oil methyl ester for wholesale Singapore trading. 1 MT is the platform input minimum; orders require compatible quality and delivery terms.","is_active":true},{"id":"9510c713-6e39-5080-add3-0a7c29657b79","name":"Bio Methanol","fuel_type":"Methanol","fuel_grade":"Bio","unit":"MT","min_lot_size":"200.00","spec_description":"Bio-methanol produced from biogenic feedstocks for marine fuel use","is_active":true},{"id":"f9b20492-b445-59cd-b292-a386d913f488","name":"e-Methanol","fuel_type":"Methanol","fuel_grade":"E","unit":"MT","min_lot_size":"200.00","spec_description":"Synthetic methanol produced from renewable hydrogen and captured CO2","is_active":true},{"id":"c4a688be-f7c2-5edc-8f93-6b34e387609c","name":"Bio Ethanol","fuel_type":"Ethanol","fuel_grade":"Bio","unit":"MT","min_lot_size":"200.00","spec_description":"Second-generation bioethanol from waste feedstocks","is_active":true},{"id":"c4cecebc-3d2d-5840-8021-57a9a11bc673","name":"Legacy Alias","fuel_type":"Biofuel","fuel_grade":"B30","unit":"MT","min_lot_size":"200.00","spec_description":"30% FAME by volume blended with 70% VLSFO; finished blend meets ISO 8217:2024 RF 380 with sulphur at most 0.50% by mass","is_active":true},{"id":"00000000-0000-4000-8000-00000000ffff","name":"B30","fuel_type":"Biofuel","fuel_grade":"B30","unit":"MT","min_lot_size":"200.00","spec_description":"30% FAME by volume blended with 70% VLSFO; finished blend meets ISO 8217:2024 RF 380 with sulphur at most 0.50% by mass","is_active":true},{"id":"8f6f34a5-2927-54a6-8564-cca40ae7cb8a","name":"Bio-LNG","fuel_type":"LNG","fuel_grade":"Bio","unit":"MT","min_lot_size":"1.00","spec_description":"Liquefied biomethane (bio-LNG) for marine fuel use, delivered to ISO 23306:2020 with a certified Proof of Sustainability.","is_active":false}]"####
        }
        _ => panic!("unknown catalog vector: {name}"),
    };
    serde_json::from_str(text).unwrap()
}

#[test]
fn project_catalog_matches_the_captured_product_vector() {
    let raw: Json = serde_json::from_slice(POLICY_BYTES).unwrap();
    let policy = load_policy(POLICY_BYTES, raw["source_sha"].as_str().unwrap()).unwrap();
    let rows: Vec<Json> = serde_json::from_value(vector("products_rows.json")).unwrap();
    let projected = project_catalog(&policy, &rows).unwrap();
    assert_eq!(projected, vector("products_expected.json"));
    // The three decoy rows (alias name, unknown UUID, inactive) stay excluded.
    // The populated corpus also deactivates one canonical product, and the
    // first decoy's target (products_expected[0]) is now B30, so the inactive
    // decoy uses products_expected[2] (Bio-LNG).
    assert_eq!(projected.as_array().unwrap().len(), 7);
}

#[test]
fn project_catalog_matches_the_captured_delivery_point_vector() {
    let raw: Json = serde_json::from_slice(POLICY_BYTES).unwrap();
    let policy = load_policy(POLICY_BYTES, raw["source_sha"].as_str().unwrap()).unwrap();
    let rows: Vec<Json> = serde_json::from_value(vector("points_rows.json")).unwrap();
    let projected = project_catalog(&policy, &rows).unwrap();
    assert_eq!(projected, vector("points_expected.json"));
    // Santos is inactive in the populated corpus.
    assert_eq!(projected.as_array().unwrap().len(), 7);
}

#[test]
fn mcp_catalog_payload_covers_list_markets_products() {
    let raw: Json = serde_json::from_slice(POLICY_BYTES).unwrap();
    let policy = load_policy(POLICY_BYTES, raw["source_sha"].as_str().unwrap()).unwrap();
    let mcp: Json = serde_json::from_slice(MCP_BYTES).unwrap();
    assert_eq!(mcp["source_sha"], raw["source_sha"]);
    let point_names: std::collections::BTreeSet<String> =
        policy.points.iter().map(|p| p.name.clone()).collect();
    let codes = mcp["products"].as_array().unwrap();
    assert_eq!(codes.len(), policy.products.len());
    for product in codes {
        assert!(
            policy
                .products
                .iter()
                .any(|p| p.code == product["code"].as_str().unwrap())
        );
        assert!(!product["unit"].as_str().unwrap().is_empty());
        assert!(!product["min_lot_size"].as_str().unwrap().is_empty());
        assert!(!product["delivery_points"].as_array().unwrap().is_empty());
        for name in product["delivery_points"].as_array().unwrap() {
            assert!(point_names.contains(name.as_str().unwrap()));
        }
    }
    assert_eq!(
        mcp["delivery_points"].as_array().unwrap().len(),
        policy.points.len()
    );
}

/// Canonical identities from `app/market_catalog.py` plus the three fixture
/// identities. None may appear in executable Rust or SQL.
fn forbidden_identity_needles() -> Vec<String> {
    let mut needles: Vec<String> = [
        "Bio-LNG",
        "UCOME B100",
        "Bio Methanol",
        "e-Methanol",
        "Bio Ethanol",
        "Synthetic Ethanol",
        "BIO_LNG",
        "UCOME_B100",
        "BIO_METHANOL",
        "E_METHANOL",
        "BIO_ETHANOL",
        "SYNTHETIC_ETHANOL",
        "8f6f34a5-2927-54a6-8564-cca40ae7cb8a",
        "e561e43f-d9b2-598e-981c-f1d28d515ddc",
        "9510c713-6e39-5080-add3-0a7c29657b79",
        "f9b20492-b445-59cd-b292-a386d913f488",
        "c4a688be-f7c2-5edc-8f93-6b34e387609c",
        "d186bffb-766d-5944-8825-989abbdcfc46",
        "78281fb3-e726-5396-802e-34b01fda21e8",
        "73835e92-820e-584b-8280-bb61c63aa28e",
        "Fixture B30 blend",
        "Fixture B100 neat",
        "Fixture UCOME B100",
        "FIXTURE_B30",
        "FIXTURE_B100",
        "FIXTURE_UCOME",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for product in fixture_policy().products {
        needles.push(product.id.to_string());
    }
    needles
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs") | Some("sql")
        ) {
            out.push(path);
        }
    }
}

/// Report `file:needle` for every needle that occurs in a labelled text source.
fn scan(needles: &[String], sources: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    for (path, text) in sources {
        for needle in needles {
            if text.contains(needle.as_str()) {
                found.push(format!("{path}:{needle}"));
            }
        }
    }
    found
}

/// The illustrative fuel category is not a catalog identity lookup. Permit
/// exactly one occurrence in the scoring core, never in another source.
fn scan_with_compliance_category(needles: &[String], sources: &[(String, String)]) -> Vec<String> {
    const SCORING: &str = "rs/read/src/compliance/scoring.rs";
    const CATEGORY: &str = "Bio-LNG";
    let mut found = scan(needles, sources);
    for (path, text) in sources {
        if path == SCORING {
            if text.matches(CATEGORY).count() == 1 {
                found.retain(|entry| entry != &format!("{SCORING}:{CATEGORY}"));
            } else {
                found.push(format!("{SCORING}:expected one illustrative category"));
            }
        }
    }
    found
}

#[test]
fn executable_sources_hold_no_product_names_codes_or_uuids() {
    let mut files = Vec::new();
    collect_sources(&Path::new(ROOT).join("rs/read/src"), &mut files);
    collect_sources(&Path::new(ROOT).join("rs/read/sql"), &mut files);
    assert!(!files.is_empty(), "no executable source was scanned");
    let sources: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            (
                path.strip_prefix(ROOT)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect();
    let needles = forbidden_identity_needles();
    assert_eq!(
        scan_with_compliance_category(&needles, &sources),
        Vec::<String>::new()
    );
}

#[test]
fn compliance_category_exception_is_exact_and_single_use() {
    let needles = forbidden_identity_needles();
    let scoring = "rs/read/src/compliance/scoring.rs";
    assert!(
        scan_with_compliance_category(&needles, &[(scoring.into(), "Bio-LNG".into())]).is_empty()
    );
    for sources in [
        vec![("rs/read/src/other.rs".into(), "Bio-LNG".into())],
        vec![(scoring.into(), "Bio-LNG Bio-LNG".into())],
        vec![(scoring.into(), "Bio-LNG BIO_LNG".into())],
        vec![(scoring.into(), "no category".into())],
    ] {
        assert!(!scan_with_compliance_category(&needles, &sources).is_empty());
    }
}

#[test]
fn the_source_scan_detects_a_planted_identity() {
    let needles = forbidden_identity_needles();
    let planted = vec![
        (
            "rs/read/src/planted.rs".to_owned(),
            format!("// {} is a product name\n", needles[0]),
        ),
        (
            "rs/read/src/planted.sql".to_owned(),
            format!("SELECT '{}'", needles[12]),
        ),
    ];
    let found = scan(&needles, &planted);
    assert_eq!(found.len(), 2, "scan missed a planted identity: {found:?}");
    assert_eq!(
        scan(
            &needles,
            &[("rs/read/src/clean.rs".into(), json!({}).to_string())]
        )
        .len(),
        0
    );
}

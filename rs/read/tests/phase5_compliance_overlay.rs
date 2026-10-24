pub use verdaxis_read::{Config, MarketPolicy, auth, market};

#[path = "../src/compliance/pricing.rs"]
pub mod pricing;
mod compliance {
    pub use crate::pricing;
}
#[path = "../src/routes/compliance_overlay.rs"]
mod overlay;

use overlay::{OverlayRequest, alcohol_family};
use pricing::AlcoholFamily;
use serde_json::json;
use uuid::Uuid;

fn fixture_policy() -> verdaxis_read::MarketPolicy {
    let bytes = include_bytes!("../../../deploy/rust/market-policy.json");
    let document: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    market::policy::load_policy(bytes, document["source_sha"].as_str().unwrap()).unwrap()
}

#[test]
fn compliance_overlay_catalog_cases() {
    let policy = fixture_policy();
    for product in &policy.products {
        let expected = match product.fuel_type.as_str() {
            "Methanol" => Some(AlcoholFamily::Methanol),
            "Ethanol" => Some(AlcoholFamily::Ethanol),
            _ => None,
        };
        assert_eq!(alcohol_family(&product.fuel_type), expected);
    }
    for label in ["methanol", "METHANOL", "Bio-Methanol", "", "unknown"] {
        assert_eq!(alcohol_family(label), None);
    }
}

#[test]
fn compliance_overlay_limits() {
    let id = Uuid::new_v4();
    let request = OverlayRequest::validate(json!({"order_ids": [id, id]})).unwrap();
    assert_eq!(request.order_ids(), &[id]);
    assert_eq!(request.year(), 2026);
    assert!(OverlayRequest::validate(json!({"order_ids": []})).is_ok());
    assert!(OverlayRequest::validate(json!({"order_ids": vec![id; 100]})).is_ok());
    assert!(OverlayRequest::validate(json!({"order_ids": vec![id; 101]})).is_err());
    for value in [
        json!({"order_ids": ["not-a-uuid"]}),
        json!({"order_ids": [], "year": 2024}),
        json!({"order_ids": [], "year": 10000}),
        json!({"order_ids": [], "year": 2026.5}),
        json!({"order_ids": [], "extra": true}),
        json!({}),
    ] {
        assert!(OverlayRequest::validate(value).is_err());
    }
    for year in [2025, 9999] {
        assert!(OverlayRequest::validate(json!({"order_ids": [], "year": year})).is_ok());
    }
}

#[test]
fn compliance_overlay_cases() {
    let _ = overlay::select_overlay;
    use overlay::{OverlayRow, project};
    use rust_decimal::Decimal;
    let first = Uuid::new_v4();
    let missing = Uuid::new_v4();
    let request = OverlayRequest::validate(json!({"order_ids": [first, missing]})).unwrap();
    let response = project(
        &request,
        vec![
            OverlayRow {
                requested_id: Some(first),
                fuel_type: Some("Methanol".into()),
                ci: Some(Decimal::from(31)),
                lcv: None,
                fleet_vessel_count: 7,
            },
            OverlayRow {
                requested_id: Some(missing),
                fuel_type: None,
                ci: None,
                lcv: None,
                fleet_vessel_count: 7,
            },
        ],
    )
    .unwrap();
    assert_eq!(response.overlays.len(), 2);
    assert!(response.overlays[&missing].is_none());
    let comparison = response.overlays[&first].as_ref().unwrap();
    assert_eq!(comparison.lcv_basis, pricing::ValueBasis::ProductDefault);
    assert_eq!(comparison.penalty_avoided_usd_per_mt, Decimal::ZERO);
    assert_eq!(
        comparison.financial_benefit_status,
        pricing::FinancialBenefitStatus::Unpriced
    );
    assert_eq!(response.assumptions.fleet_vessel_count, 7);
    let empty = OverlayRequest::validate(json!({"order_ids": []})).unwrap();
    let response = project(
        &empty,
        vec![OverlayRow {
            requested_id: None,
            fuel_type: None,
            ci: None,
            lcv: None,
            fleet_vessel_count: 7,
        }],
    )
    .unwrap();
    assert!(response.overlays.is_empty());
    assert_eq!(response.assumptions.fleet_vessel_count, 7);
}

#[test]
fn compliance_overlay_statement_shape() {
    let sql = include_str!("../sql/compliance_overlay.sql");
    assert!(sql.contains("requested_ids"));
    assert!(sql.contains("fleet_count"));
    assert!(sql.contains("LEFT JOIN eligible_orders"));
    assert_eq!(sql.matches(';').count(), 0);
}

#[test]
#[ignore = "Task 10 integration: deterministic concurrent statement snapshot fixture"]
fn compliance_overlay_snapshot_cases() {
    panic!("A deterministic writer barrier must prove the domain statement snapshot");
}

#[test]
#[ignore = "Task 2b: authenticated adapter, account/support ordering and scoped timeouts"]
fn compliance_overlay_authenticated_cases() {
    panic!("Task 2b must supply the authenticated compliance adapter before HTTP acceptance");
}

#[test]
#[ignore = "Task 2c: accepted vessel index migration and catalog gate"]
fn compliance_overlay_index_cases() {
    panic!("Task 2c must supply the accepted vessel index before route acceptance");
}

#[test]
#[ignore = "Tasks 2b and 8a: composed HTTP route and actual frontend feature fixture"]
fn compliance_overlay_client_cases() {
    panic!("Tasks 2b and 8a must land before the composed client gate");
}

#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn compliance_overlay_postgres_cases() {
    use auth::CurrentActor;
    use chrono::Utc;
    use sqlx::Row;
    let url =
        std::env::var("RUST_MARKET_TEST_DATABASE_URL").expect("disposable market fixture URL");
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    use sqlx::Connection;
    let policy = fixture_policy();
    let config = Config {
        environment: "test".into(),
        release_sha: std::env::var("RELEASE_SHA").unwrap(),
        database_url: url,
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
        migration_revision: "kycg_20261014_staging".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    };
    verdaxis_read::validate_config(&config).unwrap();
    let clock = market::visibility::VisibilityClock::at(
        std::env::var("RUST_MARKET_TEST_CLOCK")
            .ok()
            .map(|value| value.parse().unwrap())
            .unwrap_or_else(Utc::now),
    );
    let visible = market::visibility::select_public_order_ids(
        &mut connection,
        &policy,
        &clock,
        &config,
        &market::visibility::PublicOrderFilters {
            side: Some("ASK"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!visible.is_empty(), "fixture must contain public ASKs");
    let request =
        OverlayRequest::validate(json!({"order_ids": &visible[..visible.len().min(100)]})).unwrap();
    let actor = CurrentActor {
        id: Uuid::new_v4(),
        organization_id: None,
        role: Some("BUYER".into()),
        email_verified: true,
    };
    let response =
        overlay::select_overlay(&mut connection, &policy, &clock, &config, &actor, &request)
            .await
            .unwrap();
    assert!(
        response.overlays.values().any(Option::is_some),
        "fixture must exercise priced lifecycle values"
    );
    for id in request.order_ids() {
        let row = sqlx::query("SELECT p.fuel_type, o.carbon_intensity_gco2_mj AS ci, o.energy_density_mj_kg AS lcv FROM orderbook_orders o JOIN products p ON p.id=o.product_id WHERE o.id=$1").bind(id).fetch_one(&mut connection).await.unwrap();
        let fuel: String = row.get("fuel_type");
        let expected = pricing::LifecycleOverlayCore::compute(
            alcohol_family(&fuel),
            row.get("ci"),
            row.get("lcv"),
        )
        .unwrap();
        assert_eq!(response.overlays[id], expected);
    }
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var("RUST_OVERLAY_TEST_CASES").expect("overlay fixture manifest path"),
        )
        .expect("overlay fixture manifest must exist"),
    )
    .expect("valid overlay fixture manifest");
    let ids = |key: &str| -> Vec<Uuid> {
        fixture[key]
            .as_array()
            .expect("required fixture ID list")
            .iter()
            .map(|value| value.as_str().unwrap().parse().unwrap())
            .collect()
    };
    let qualified = ids("visible_ids");
    let hidden = ids("hidden_ids");
    assert!(
        qualified.len() >= 2,
        "two qualified fixture orders required"
    );
    assert!(hidden.len() >= 2, "hidden ASK and BID fixtures required");
    let mut hidden_ask = false;
    let mut hidden_bid = false;
    for id in &hidden {
        let row = sqlx::query(
            "SELECT o.side::text AS side, p.fuel_type, o.carbon_intensity_gco2_mj AS ci, \
             o.energy_density_mj_kg AS lcv FROM orderbook_orders o \
             JOIN products p ON p.id=o.product_id WHERE o.id=$1",
        )
        .bind(id)
        .fetch_one(&mut connection)
        .await
        .expect("known hidden fixture order must exist");
        let side: String = row.get("side");
        hidden_ask |= side == "ASK";
        hidden_bid |= side == "BID";
        let fuel: String = row.get("fuel_type");
        assert!(
            pricing::LifecycleOverlayCore::compute(
                alcohol_family(&fuel),
                row.get("ci"),
                row.get("lcv"),
            )
            .unwrap()
            .is_some(),
            "hidden fixture must have computable values"
        );
    }
    assert!(hidden_ask && hidden_bid, "known ASK and BID required");
    let absent = Uuid::new_v4();
    let mixed: Vec<Uuid> = qualified
        .iter()
        .chain(&hidden)
        .copied()
        .chain([absent])
        .collect();
    let request = OverlayRequest::validate(json!({"order_ids": mixed})).unwrap();
    let response =
        overlay::select_overlay(&mut connection, &policy, &clock, &config, &actor, &request)
            .await
            .unwrap();
    assert_eq!(response.overlays.len(), request.order_ids().len());
    for id in hidden.iter().chain([&absent]) {
        assert_eq!(response.overlays[id], None, "null overlay for {id}");
    }
    for id in &qualified {
        assert!(visible.contains(id), "qualified fixture must be public");
        assert!(
            response.overlays[id].is_some(),
            "qualified overlay for {id}"
        );
    }
    assert_ne!(
        response.overlays[&qualified[0]], response.overlays[&qualified[1]],
        "qualified fixtures must have distinct lifecycle values"
    );
    let scopes = fixture["scopes"].as_array().expect("required actor scopes");
    assert_eq!(scopes.len(), 5, "all actor scopes are required");
    for (scope, (name, role, count)) in scopes.iter().zip([
        ("administrator", "ADMIN", 6),
        ("member", "BUYER", 2),
        ("other_member", "BUYER", 3),
        ("empty_member", "BUYER", 0),
        ("no_organization", "BUYER", 0),
    ]) {
        assert_eq!(scope["name"], name);
        assert_eq!(scope["role"], role);
        assert_eq!(scope["count"], count);
        let organization_id = scope["organization_id"]
            .as_str()
            .map(|value| value.parse().unwrap());
        if matches!(name, "member" | "other_member" | "empty_member") {
            assert!(organization_id.is_some(), "member organization is required");
        } else {
            assert!(organization_id.is_none());
        }
        let scoped_actor = CurrentActor {
            organization_id,
            role: Some(scope["role"].as_str().unwrap().into()),
            ..actor.clone()
        };
        let expected_count = scope["count"].as_u64().expect("exact fixture count");
        for ids in [vec![], mixed.clone(), vec![qualified[0]; 100]] {
            let request = OverlayRequest::validate(json!({"order_ids": ids})).unwrap();
            let response = overlay::select_overlay(
                &mut connection,
                &policy,
                &clock,
                &config,
                &scoped_actor,
                &request,
            )
            .await
            .unwrap();
            assert_eq!(response.overlays.len(), request.order_ids().len());
            assert_eq!(
                response.assumptions.fleet_vessel_count,
                expected_count,
                "fleet scope {} with {} requested IDs",
                scope["name"],
                request.order_ids().len()
            );
        }
    }
}

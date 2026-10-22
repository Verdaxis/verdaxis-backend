pub use verdaxis_read::{Config, auth, market};

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
    assert_eq!(comparison.financial_benefit_status, pricing::FinancialBenefitStatus::Unpriced);
    assert_eq!(response.assumptions.fleet_vessel_count, 7);
    let empty = OverlayRequest::validate(json!({"order_ids": []})).unwrap();
    let response = project(&empty, vec![OverlayRow {
        requested_id: None, fuel_type: None, ci: None, lcv: None, fleet_vessel_count: 7,
    }]).unwrap();
    assert!(response.overlays.is_empty());
    assert_eq!(response.assumptions.fleet_vessel_count, 7);
}

#[test]
fn compliance_overlay_snapshot_cases() {
    let sql = include_str!("../sql/compliance_overlay.sql");
    assert!(sql.contains("requested_ids"));
    assert!(sql.contains("fleet_count"));
    assert!(sql.contains("LEFT JOIN eligible_orders"));
    assert_eq!(sql.matches(';').count(), 0);
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
    use auth::actor::CurrentActor;
    use chrono::Utc;
    use sqlx::Row;
    let url = std::env::var("RUST_MARKET_TEST_DATABASE_URL").expect("disposable market fixture URL");
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    use sqlx::Connection;
    let policy = fixture_policy();
    let config = Config {
        environment: "test".into(), release_sha: std::env::var("RELEASE_SHA").unwrap(),
        database_url: url, kyc_execution_gate_enabled: true,
        db_session_time_zone: "Asia/Singapore".into(),
        migration_revision: "kycg_20261014_staging".into(),
        bind: "127.0.0.1:0".parse().unwrap(), read_admission_slots: 16,
        pool_max: 17, python_budget: 52, configured_max_connections: 100, prepared_budget: None,
    };
    verdaxis_read::validate_config(&config).unwrap();
    let clock = market::visibility::VisibilityClock::at(Utc::now());
    let id: Uuid = sqlx::query("SELECT id FROM orderbook_orders WHERE side::text='ASK' LIMIT 1")
        .fetch_one(&mut connection).await.unwrap().get("id");
    let absent = Uuid::new_v4();
    let actor = CurrentActor { id: Uuid::new_v4(), organization_id: None, role: Some("BUYER".into()), email_verified: true };
    for ids in [vec![], vec![id, absent], vec![id; 100]] {
        let request = OverlayRequest::validate(json!({"order_ids": ids})).unwrap();
        let response = overlay::select_overlay(&mut connection, &policy, &clock, &config, &actor, &request).await.unwrap();
        assert_eq!(response.overlays.len(), request.order_ids().len());
        assert_eq!(response.assumptions.fleet_vessel_count, 0);
        if response.overlays.contains_key(&absent) { assert!(response.overlays[&absent].is_none()); }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM vessels").fetch_one(&mut connection).await.unwrap();
    let request = OverlayRequest::validate(json!({"order_ids": []})).unwrap();
    let admin = CurrentActor { role: Some("ADMIN".into()), ..actor.clone() };
    let response = overlay::select_overlay(&mut connection, &policy, &clock, &config, &admin, &request).await.unwrap();
    assert_eq!(response.assumptions.fleet_vessel_count, count as u64);
    let org: Option<Uuid> = sqlx::query_scalar("SELECT organization_id FROM vessels WHERE organization_id IS NOT NULL LIMIT 1").fetch_optional(&mut connection).await.unwrap();
    if let Some(org) = org {
        let own_count: i64 = sqlx::query_scalar("SELECT count(*) FROM vessels WHERE organization_id=$1").bind(org).fetch_one(&mut connection).await.unwrap();
        let owner = CurrentActor { organization_id: Some(org), ..actor };
        let response = overlay::select_overlay(&mut connection, &policy, &clock, &config, &owner, &request).await.unwrap();
        assert_eq!(response.assumptions.fleet_vessel_count, own_count as u64);
    }
}

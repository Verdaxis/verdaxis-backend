use serde_json::Value;
use verdaxis_read::auth::abilities::{build_abilities, Facts};

fn reason(facts: &Facts, action: &str) -> Option<String> {
    let value = serde_json::to_value(build_abilities(facts)).unwrap();
    value["abilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == action)
        .unwrap()["reason_code"]
        .as_str()
        .map(str::to_owned)
}

#[test]
fn execution_denials_precede_action_roles_and_provenance() {
    let mut facts = Facts::approved_buyer();
    facts.email_verified = false;
    facts.organization_status = Some("REJECTED".into());
    facts.kyc_status = "REJECTED".into();
    assert_eq!(
        reason(&facts, "place_ask_order").as_deref(),
        Some("EMAIL_NOT_VERIFIED")
    );
    facts.email_verified = true;
    assert_eq!(
        reason(&facts, "place_ask_order").as_deref(),
        Some("ORGANIZATION_NOT_APPROVED")
    );
    facts.organization_status = Some("APPROVED".into());
    assert_eq!(
        reason(&facts, "place_ask_order").as_deref(),
        Some("KYC_REJECTED")
    );
    facts.kyc_status = "APPROVED".into();
    assert_eq!(
        reason(&facts, "place_ask_order").as_deref(),
        Some("SUPPLIER_ROLE_REQUIRED")
    );
    facts.provenance = "UNKNOWN".into();
    assert_eq!(
        reason(&facts, "hit_bid_order").as_deref(),
        Some("SUPPLIER_ROLE_REQUIRED")
    );
    assert_eq!(
        reason(&facts, "hit_ask_order").as_deref(),
        Some("ORGANIZATION_PROVENANCE_UNVERIFIED")
    );
}

#[test]
fn retired_actions_and_cleanup_do_not_take_execution_gates() {
    let mut facts = Facts::approved_buyer();
    facts.kyc_status = "REJECTED".into();
    facts.organization_status = Some("REJECTED".into());
    for action in ["cancel_order", "deliver_trade"] {
        assert_eq!(reason(&facts, action), None);
    }
    for action in ["create_rfq", "quote_rfq"] {
        assert_eq!(reason(&facts, action).as_deref(), Some("ACTION_RETIRED"));
    }
    let payload: Value = serde_json::to_value(build_abilities(&facts)).unwrap();
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["abilities"].as_array().unwrap().len(), 23);
}

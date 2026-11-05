use serde_json::Value;
use verdaxis_read::auth::abilities::{Facts, build_abilities};

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
        reason(&facts, "hit_bid_order").as_deref(),
        Some("EMAIL_NOT_VERIFIED")
    );
    facts.email_verified = true;
    assert_eq!(
        reason(&facts, "hit_bid_order").as_deref(),
        Some("ORGANIZATION_NOT_APPROVED")
    );
    facts.organization_status = Some("APPROVED".into());
    assert_eq!(
        reason(&facts, "hit_bid_order").as_deref(),
        Some("KYC_REJECTED")
    );
    facts.kyc_status = "APPROVED".into();
    assert_eq!(
        reason(&facts, "hit_bid_order").as_deref(),
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
fn create_checks_kyc_before_organization_and_email() {
    let mut f = Facts::approved_buyer();
    f.email_verified = false;
    f.organization_status = Some("REJECTED".into());
    f.kyc_status = "PENDING".into();
    for action in ["place_bid_order", "place_ask_order"] {
        assert_eq!(reason(&f, action).as_deref(), Some("KYC_REQUIRED"));
    }
}

#[test]
fn every_reason_code_has_a_reachable_policy_case() {
    let base = Facts::approved_buyer();
    let cases: Vec<(&str, &str, Facts)> = vec![
        (
            "publish_inventory",
            "ADMIN_CANNOT_EXECUTE",
            Facts {
                role: Some("ADMIN".into()),
                ..base.clone()
            },
        ),
        (
            "publish_inventory",
            "ROLE_NOT_ASSIGNED",
            Facts {
                role: None,
                ..base.clone()
            },
        ),
        (
            "publish_inventory",
            "ORGANIZATION_REQUIRED",
            Facts {
                has_organization: false,
                ..base.clone()
            },
        ),
        (
            "publish_inventory",
            "ACCOUNT_NOT_APPROVED",
            Facts {
                status: "PENDING".into(),
                ..base.clone()
            },
        ),
        (
            "publish_inventory",
            "EMAIL_NOT_VERIFIED",
            Facts {
                email_verified: false,
                ..base.clone()
            },
        ),
        (
            "publish_inventory",
            "PASSWORD_CHANGE_REQUIRED",
            Facts {
                must_change_password: true,
                ..base.clone()
            },
        ),
        (
            "publish_inventory",
            "ORGANIZATION_NOT_APPROVED",
            Facts {
                organization_status: Some("REJECTED".into()),
                ..base.clone()
            },
        ),
        (
            "place_bid_order",
            "KYC_REQUIRED",
            Facts {
                kyc_status: "PENDING".into(),
                ..base.clone()
            },
        ),
        (
            "place_bid_order",
            "KYC_REJECTED",
            Facts {
                kyc_status: "REJECTED".into(),
                ..base.clone()
            },
        ),
        (
            "manage_inventory",
            "KYC_ORGANIZATION_MISMATCH",
            Facts {
                kyc_matches: false,
                ..base.clone()
            },
        ),
        (
            "place_bid_order",
            "BUYER_ROLE_REQUIRED",
            Facts {
                role: Some("SUPPLIER".into()),
                ..base.clone()
            },
        ),
        ("place_ask_order", "SUPPLIER_ROLE_REQUIRED", base.clone()),
        ("view_feedback", "ADMIN_ROLE_REQUIRED", base.clone()),
        (
            "hit_ask_order",
            "ORGANIZATION_PROVENANCE_UNVERIFIED",
            Facts {
                provenance: "DEMO".into(),
                ..base.clone()
            },
        ),
        (
            "cancel_order",
            "LEGACY_ADMIN_MUTATION_RETIRED",
            Facts {
                role: Some("ADMIN".into()),
                support_enabled: true,
                ..base.clone()
            },
        ),
        (
            "market_support_access",
            "MARKET_SUPPORT_DISABLED",
            base.clone(),
        ),
        (
            "market_support_access",
            "MARKET_SUPPORT_CAPABILITY_REQUIRED",
            Facts {
                role: Some("ADMIN".into()),
                support_enabled: true,
                ..base.clone()
            },
        ),
        ("create_rfq", "ACTION_RETIRED", base),
    ];
    for (action, code, facts) in cases {
        assert_eq!(reason(&facts, action).as_deref(), Some(code), "{action}");
    }
}

#[test]
fn support_switch_role_and_capabilities_are_ordered() {
    let mut f = Facts::approved_buyer();
    assert_eq!(
        reason(&f, "market_support_access").as_deref(),
        Some("MARKET_SUPPORT_DISABLED")
    );
    f.support_enabled = true;
    assert_eq!(
        reason(&f, "market_support_access").as_deref(),
        Some("ADMIN_ROLE_REQUIRED")
    );
    f.role = Some("ADMIN".into());
    f.listings = true;
    assert_eq!(reason(&f, "market_support_access"), None);
    assert_eq!(
        reason(&f, "market_support_assisted_orders").as_deref(),
        Some("MARKET_SUPPORT_CAPABILITY_REQUIRED")
    );
    f.authorizations = true;
    assert_eq!(reason(&f, "market_support_assisted_orders"), None);
    for action in [
        "view_admin_analytics",
        "view_audit_log",
        "view_user_activity",
        "view_feedback",
        "manage_subscriptions",
        "manage_accounts",
        "review_kyc",
        "manage_commissions",
    ] {
        assert_eq!(reason(&f, action), None);
    }
    for action in ["amend_order", "cancel_order"] {
        assert_eq!(
            reason(&f, action).as_deref(),
            Some("LEGACY_ADMIN_MUTATION_RETIRED")
        );
    }
}

#[test]
fn grace_and_advisory_switch_share_execution_truth_table() {
    let mut f = Facts::approved_buyer();
    f.kyc_status = "PENDING".into();
    f.grace = true;
    for action in [
        "place_bid_order",
        "amend_order",
        "hit_ask_order",
        "confirm_trade",
    ] {
        assert_eq!(reason(&f, action), None);
    }
    f.grace = false;
    assert_eq!(reason(&f, "confirm_trade").as_deref(), Some("KYC_REQUIRED"));
    f.execution_gate_enabled = false;
    assert_eq!(reason(&f, "confirm_trade"), None);
    f.kyc_matches = false;
    assert_eq!(reason(&f, "confirm_trade").as_deref(), Some("KYC_REQUIRED"));
    f.kyc_status = "REJECTED".into();
    f.grace = true;
    assert_eq!(reason(&f, "confirm_trade").as_deref(), Some("KYC_REJECTED"));
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

use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::market_support::{
    SupportSettings,
    contexts::{ContextCreate, ContextRecord, ContextStatus, Organization, Principal},
};

#[test]
fn context_reference_checks_raw_length_then_trims() {
    let organization = Uuid::new_v4();
    for raw in ["", "ab", "   ", &"x".repeat(201)] {
        assert!(
            ContextCreate::parse(json!({"organization_id":organization,"support_reference":raw}))
                .is_err()
        );
    }
    let body = ContextCreate::parse(
        json!({"organization_id":organization,"support_reference":"\u{001c} ab \u{001f}"}),
    )
    .unwrap();
    assert_eq!(body.support_reference, "ab");
    assert!(!body.confirm_replacement);
    assert_eq!(body.organization_id, organization);
    for value in [json!(1), json!(null), json!(true)] {
        assert!(
            ContextCreate::parse(json!({"organization_id":organization,"support_reference":value}))
                .is_err()
        );
    }
    assert!(ContextCreate::parse(json!({"organization_id":organization,"support_reference":"case","confirm_replacement":"true"})).is_err());
}

#[test]
fn context_ttl_has_a_default_and_validated_bounds() {
    assert_eq!(SupportSettings::new(true, "").context_ttl_minutes(), 60);
    for minutes in [1, 37, 1440] {
        let settings = SupportSettings::new(true, "")
            .with_context_ttl_minutes(minutes)
            .unwrap();
        assert_eq!(settings.context_ttl_minutes(), minutes);
    }
    for minutes in [0, -1, 1441] {
        assert!(
            SupportSettings::new(true, "")
                .with_context_ttl_minutes(minutes)
                .is_err()
        );
    }
}

#[test]
fn context_status_enum_has_four_members() {
    for text in ["ACTIVE", "EXITED", "EXPIRED", "REVOKED"] {
        let state: ContextStatus = serde_json::from_value(json!(text)).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), text);
    }
    assert!(serde_json::from_value::<ContextStatus>(json!("REPLACED")).is_err());
    assert!(serde_json::from_value::<ContextStatus>(json!("active")).is_err());
}

#[test]
fn context_projection_keeps_actor_name_whitespace_and_requires_identities() {
    let actor = Uuid::new_v4();
    let organization = Organization {
        id: Uuid::new_v4(),
        name: "Customer".into(),
        domain: None,
        organization_type: "FUEL_SUPPLIER".into(),
    };
    let principal = Principal::new(
        actor,
        "actor@example.test".into(),
        Some(" ".into()),
        None,
        false,
    );
    assert_eq!(principal.name, " ");
    let trimmed = Principal::new(
        actor,
        "actor@example.test".into(),
        Some(" ".into()),
        None,
        true,
    );
    assert_eq!(trimmed.name, "actor@example.test");
    let named = Principal::new(
        actor,
        "actor@example.test".into(),
        Some("First".into()),
        Some("Last".into()),
        false,
    );
    assert_eq!(named.name, "First Last");
    let now = Utc.with_ymd_and_hms(2026, 10, 27, 0, 0, 0).unwrap();
    let record = ContextRecord {
        id: Uuid::new_v4(),
        actor_user_id: actor,
        organization_id: organization.id,
        support_reference: "case".into(),
        scope: "ASSISTED_ORDER_ENTRY".into(),
        started_at: now,
        expires_at: now + Duration::hours(1),
        ended_at: None,
        status: ContextStatus::Active,
        version: 1,
    };
    let response = record
        .response(Some(&organization), Some(&principal))
        .unwrap();
    assert_eq!(response["actor"]["name"], " ");
    assert_eq!(response["organization"]["domain"], json!(null));
    assert_eq!(response["started_at"], "2026-10-27T00:00:00Z");
    assert_eq!(response["ended_at"], json!(null));
    assert_eq!(response.as_object().unwrap().len(), 12);
    for (org, person) in [(None, Some(&principal)), (Some(&organization), None)] {
        let error = record.response(org, person).unwrap_err();
        assert_eq!(error.status.as_u16(), 409);
        assert_eq!(error.code.as_str(), "MARKET_SUPPORT_IDENTITIES_UNAVAILABLE");
    }
}

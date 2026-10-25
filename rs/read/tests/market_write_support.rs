use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::market_support::{CapabilityGrant, Page, SupportSettings};

#[test]
fn bootstrap_ids_are_exact_uuid_memberships() {
    let actor = Uuid::new_v4();
    let settings = SupportSettings::new(true, &format!("invalid, {actor}, ,{actor}"));
    assert!(settings.is_bootstrap(actor));
    assert!(!settings.is_bootstrap(Uuid::new_v4()));
    assert!(!SupportSettings::new(true, "").is_bootstrap(actor));
}

#[test]
fn support_page_defaults_and_bounds_are_explicit() {
    let page = Page::parse(None).unwrap();
    assert_eq!((page.skip, page.limit), (0, 25));
    assert!(Page::parse(Some("skip=-1")).is_err());
    assert!(Page::parse(Some("limit=0")).is_err());
    assert!(Page::parse(Some("limit=101")).is_err());
    assert!(Page::parse(Some(&format!("query={}", "x".repeat(101)))).is_err());
    let page = Page::parse(Some("skip=2&limit=100&query=+Example+")).unwrap();
    assert_eq!((page.skip, page.limit), (2, 100));
    assert_eq!(page.query.as_deref(), Some("Example"));
}

#[test]
fn capability_input_validates_names_reason_and_future_expiry() {
    let now = Utc::now();
    let mut body = json!({"user_id":Uuid::new_v4(),"capability":"MARKET_SUPPORT_LISTINGS","reason":"Customer support"});
    assert!(CapabilityGrant::parse(body.clone(), now).is_ok());
    body["capability"] = json!("OTHER");
    assert!(CapabilityGrant::parse(body.clone(), now).is_err());
    body["capability"] = json!("MARKET_SUPPORT_AUTHORIZATIONS");
    for reason in ["", "  ", &"x".repeat(501)] {
        body["reason"] = json!(reason);
        assert!(CapabilityGrant::parse(body.clone(), now).is_err());
    }
    body["reason"] = json!("Support request");
    body["expires_at"] = json!(now - Duration::seconds(1));
    assert!(CapabilityGrant::parse(body.clone(), now).is_err());
    body["expires_at"] = json!(now + Duration::hours(1));
    assert!(CapabilityGrant::parse(body, now).is_ok());
}

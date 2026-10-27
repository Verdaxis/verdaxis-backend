use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::market_support::{CapabilityGrant, Page, SupportSettings, reason};

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

#[test]
fn reason_length_is_checked_before_surrounding_whitespace_is_removed() {
    assert_eq!(reason("   ").unwrap(), "");
    assert_eq!(reason(" ab ").unwrap(), "ab");
    assert_eq!(reason("\u{1c}abc\u{1f}").unwrap(), "abc");
    assert!(reason("ab").is_err());
    assert!(reason(&format!(" {} ", "x".repeat(499))).is_err());
    assert_eq!(reason(&"x".repeat(500)).unwrap().chars().count(), 500);
    let body =
        json!({"user_id":Uuid::new_v4(),"capability":"MARKET_SUPPORT_LISTINGS","reason":"   "});
    assert_eq!(CapabilityGrant::parse(body, Utc::now()).unwrap().reason, "");
}

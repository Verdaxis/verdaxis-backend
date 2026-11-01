//! Request contract of the administrator obligation list and the
//! responsible-person reassignment. No database is needed.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::market_reassignment::{
    AUDIT_ACTION, CASE_REFERENCE_MAX_LENGTH, CASE_REFERENCE_PREFIXES, CaseReference, EVENT_TYPE,
    MarketErrorCode, OBLIGATION_PAGE_LIMIT, ObligationPage, REQUEST_BODY_LIMIT_BYTES,
    ReassignRequest, ResponsibleSide,
};

const CURRENT: &str = "00000000-0000-0000-0000-000000002001";
const REPLACEMENT: &str = "00000000-0000-0000-0000-000000002002";

fn body() -> Value {
    json!({
        "side": "SELLER",
        "replacement_user_id": REPLACEMENT,
        "evidence_case_ref": "CASE-2026/0042",
        "expected_current_user_id": CURRENT,
    })
}

fn parse(value: &Value) -> Result<ReassignRequest, MarketErrorCode> {
    ReassignRequest::parse(value.to_string().as_bytes()).map_err(|error| error.code)
}

#[test]
fn request_contract_is_exact() {
    let request = parse(&body()).expect("the documented body is accepted");
    assert_eq!(request.side, ResponsibleSide::Seller);
    assert_eq!(
        request.replacement_user_id,
        REPLACEMENT.parse::<Uuid>().unwrap()
    );
    assert_eq!(
        request.expected_current_user_id,
        CURRENT.parse::<Uuid>().unwrap()
    );
    assert_eq!(request.evidence_case_ref.as_str(), "CASE-2026/0042");
    let mut buyer = body();
    buyer["side"] = json!("BUYER");
    assert_eq!(parse(&buyer).unwrap().side, ResponsibleSide::Buyer);

    let refused = |value: Value| {
        assert_eq!(
            parse(&value).unwrap_err(),
            MarketErrorCode::ValidationFailed,
            "{value}"
        );
    };
    // Every field is required, none may be null, and no other field is accepted.
    for field in [
        "side",
        "replacement_user_id",
        "evidence_case_ref",
        "expected_current_user_id",
    ] {
        let mut missing = body();
        missing.as_object_mut().unwrap().remove(field);
        refused(missing);
        let mut null = body();
        null[field] = Value::Null;
        refused(null);
    }
    let mut extra = body();
    extra["reason"] = json!("left the company");
    refused(extra);
    for side in ["buyer", "BOTH", "", "ADMIN"] {
        let mut wrong = body();
        wrong["side"] = json!(side);
        refused(wrong);
    }
    let mut not_uuid = body();
    not_uuid["replacement_user_id"] = json!("someone@example.test");
    refused(not_uuid);
    // Naming the current person again changes nothing and is refused.
    let mut same = body();
    same["replacement_user_id"] = json!(CURRENT);
    refused(same);
    // A body above the bound is refused before parsing.
    let mut padded = body().to_string();
    padded.push_str(&" ".repeat(REQUEST_BODY_LIMIT_BYTES));
    assert_eq!(
        ReassignRequest::parse(padded.as_bytes()).unwrap_err().code,
        MarketErrorCode::ValidationFailed
    );
    for text in ["", "not json", "[]", "null"] {
        assert!(ReassignRequest::parse(text.as_bytes()).is_err(), "{text}");
    }
}

#[test]
fn case_reference_is_an_opaque_ticket_identifier() {
    for accepted in [
        "CASE-2026/0042",
        "SEC-2026-11-0001",
        "REQ-1",
        "TICKET-123456789012",
    ] {
        assert_eq!(CaseReference::parse(accepted).unwrap().as_str(), accepted);
    }
    for prefix in CASE_REFERENCE_PREFIXES {
        assert!(CaseReference::parse(&format!("{prefix}-7")).is_ok());
    }
    let too_long = format!("CASE-{}", "1".repeat(CASE_REFERENCE_MAX_LENGTH));
    for refused in [
        "",
        "CASE",
        "CASE-",
        "case-1",
        "HR-2026/1",
        "CASE-Jane Doe",
        "CASE-jane@example.test",
        "CASE-2026/",
        "CASE-1/2/3/4",
        "CASE-1234567890123",
        "CASE-２０２６",
        "CASE-2026 0042",
        too_long.as_str(),
    ] {
        assert!(CaseReference::parse(refused).is_err(), "{refused}");
    }
    let mut request = body();
    request["evidence_case_ref"] = json!("Dismissed for misconduct");
    assert_eq!(
        parse(&request).unwrap_err(),
        MarketErrorCode::ValidationFailed
    );
    // The reference never appears in debug output or a log line built from it.
    let parsed = parse(&body()).unwrap();
    let debug = format!("{parsed:?}");
    assert!(!debug.contains("2026/0042"), "{debug}");
    assert!(debug.contains("REDACTED"), "{debug}");
}

#[test]
fn obligation_page_is_bounded() {
    assert_eq!(
        ObligationPage::parse(None).unwrap(),
        ObligationPage {
            skip: 0,
            limit: OBLIGATION_PAGE_LIMIT
        }
    );
    assert_eq!(
        ObligationPage::parse(Some("skip=5&limit=1")).unwrap(),
        ObligationPage { skip: 5, limit: 1 }
    );
    assert_eq!(OBLIGATION_PAGE_LIMIT, 100);
    for refused in ["limit=0", "limit=101", "skip=-1", "limit=x", "skip=1.5"] {
        assert!(ObligationPage::parse(Some(refused)).is_err(), "{refused}");
    }
}

#[test]
fn refusal_codes_are_stable() {
    for (code, name, status) in [
        (
            MarketErrorCode::TradeNotReassignable,
            "TRADE_NOT_REASSIGNABLE",
            StatusCode::CONFLICT,
        ),
        (
            MarketErrorCode::ReassignmentStale,
            "REASSIGNMENT_STALE",
            StatusCode::CONFLICT,
        ),
        (
            MarketErrorCode::ReassignmentCurrentStillMember,
            "REASSIGNMENT_CURRENT_STILL_MEMBER",
            StatusCode::CONFLICT,
        ),
        (
            MarketErrorCode::ReassignmentReplacementIneligible,
            "REASSIGNMENT_REPLACEMENT_INELIGIBLE",
            StatusCode::CONFLICT,
        ),
        (
            MarketErrorCode::ReassignmentManualResolutionRequired,
            "REASSIGNMENT_MANUAL_RESOLUTION_REQUIRED",
            StatusCode::CONFLICT,
        ),
        (
            MarketErrorCode::UserNotFound,
            "USER_NOT_FOUND",
            StatusCode::NOT_FOUND,
        ),
        (
            MarketErrorCode::TradeFrozen,
            "TRADE_FROZEN",
            StatusCode::CONFLICT,
        ),
        (
            MarketErrorCode::SupportForbidden,
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
        ),
    ] {
        assert_eq!(code.as_str(), name);
        assert_eq!(code.status(), status, "{name}");
    }
    // The audit and event names carry no person, case or reason.
    assert_eq!(AUDIT_ACTION, "trade.responsibility_reassigned");
    assert_eq!(EVENT_TYPE, "trade_responsibility_reassigned");
}

/// The served market composition never names the reassignment component;
/// only the separate dormant composition function does.
#[test]
fn serving_composition_leaves_the_routes_unmounted() {
    let source = include_str!("../src/runtime/market_write.rs");
    let start = source
        .find("pub(crate) fn routes(")
        .expect("the serving market composition");
    let end = source[start..]
        .find("\n}\n")
        .map(|offset| start + offset)
        .expect("the end of the serving composition");
    let serving = &source[start..end];
    assert!(
        serving.contains("freeze::router"),
        "the scan reads the right function"
    );
    assert!(
        !serving.contains("reassignment"),
        "the serving composition mounts the reassignment routes"
    );
    assert!(source.contains("pub fn reassignment_routes("));
    let http = include_str!("../src/runtime/http.rs");
    assert!(!http.contains("reassignment"));
    let phase4 = include_str!("../src/runtime/phase4.rs");
    assert!(!phase4.contains("reassignment"));
}

/// The reassignment and the account offboarding command accept exactly the
/// same case references: both use one type.
#[test]
fn case_reference_is_shared_with_the_offboarding_command() {
    let reference: verdaxis_read::business::offboarding::CaseReference =
        CaseReference::parse("CASE-2026/0042").unwrap();
    assert_eq!(reference.as_str(), "CASE-2026/0042");
    assert_eq!(
        CASE_REFERENCE_PREFIXES,
        verdaxis_read::business::offboarding::CASE_REFERENCE_PREFIXES
    );
}

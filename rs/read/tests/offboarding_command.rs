//! Request contract and composition of the account offboarding command.
//!
//! These cases need no database. The PostgreSQL behavior is in
//! `offboarding_command_postgres.rs`.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::business::offboarding::{
    AUDIT_ACTION, CASE_REFERENCE_MAX_LENGTH, CASE_REFERENCE_PREFIXES, CaseReference,
    OFFBOARD_ROUTE, OffboardRequest, PREVIEW_COUNT_LIMIT, PREVIEW_ROUTE, REASON_CODES,
    REQUEST_BODY_LIMIT_BYTES, ReasonCode,
};

fn body(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

fn valid() -> Value {
    json!({
        "reason_code": "EMPLOYMENT_ENDED",
        "evidence_case_ref": "CASE-2026/0042",
        "urgent": false,
        "expected_organization_id": null,
    })
}

fn refused(bytes: &[u8]) -> (StatusCode, String) {
    let error = OffboardRequest::parse(bytes).expect_err("the body must be refused");
    let code = error.detail["code"].as_str().unwrap().to_owned();
    (error.status, code)
}

#[test]
fn request_validation_accepts_the_contract() {
    let parsed = OffboardRequest::parse(&body(valid())).unwrap();
    assert_eq!(parsed.reason_code.as_str(), "EMPLOYMENT_ENDED");
    assert_eq!(parsed.evidence_case_ref.as_str(), "CASE-2026/0042");
    assert!(!parsed.urgent);
    assert_eq!(parsed.expected_organization_id, None);

    let organization = Uuid::new_v4();
    let mut member = valid();
    member["expected_organization_id"] = json!(organization);
    member["urgent"] = json!(true);
    member["reason_code"] = json!("SECURITY_CONCERN");
    let parsed = OffboardRequest::parse(&body(member)).unwrap();
    assert_eq!(parsed.expected_organization_id, Some(organization));
    assert!(parsed.urgent);
    for code in REASON_CODES {
        assert_eq!(ReasonCode::parse(code).unwrap().as_str(), code);
    }
}

#[test]
fn request_validation_requires_an_explicit_organization_field() {
    let mut missing = valid();
    missing
        .as_object_mut()
        .unwrap()
        .remove("expected_organization_id");
    assert_eq!(
        refused(&body(missing)),
        (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED".into())
    );
    for wrong in [json!("not-a-uuid"), json!(7), json!(true), json!({})] {
        let mut value = valid();
        value["expected_organization_id"] = wrong;
        assert_eq!(refused(&body(value)).1, "VALIDATION_FAILED");
    }
}

#[test]
fn request_validation_rejects_invalid_reasons_and_fields() {
    for reason in [
        json!("employment_ended"),
        json!("OTHER"),
        json!(""),
        json!(null),
        json!(1),
    ] {
        let mut value = valid();
        value["reason_code"] = reason;
        assert_eq!(refused(&body(value)).1, "VALIDATION_FAILED");
    }
    for urgent in [json!("false"), json!(null), json!(0)] {
        let mut value = valid();
        value["urgent"] = urgent;
        assert_eq!(refused(&body(value)).1, "VALIDATION_FAILED");
    }
    // A free-text field cannot ride along with the four contract fields.
    let mut extra = valid();
    extra["notes"] = json!("left after a dispute with the manager");
    assert_eq!(refused(&body(extra)).1, "VALIDATION_FAILED");
    for shape in [&b""[..], b"null", b"[]", b"\"text\"", b"{"] {
        assert_eq!(refused(shape).1, "VALIDATION_FAILED");
    }
}

#[test]
fn request_validation_rejects_sensitive_or_oversized_case_references() {
    // An allowlisted ticket prefix, then digit groups only: no name, email or
    // narrative fits the grammar.
    for prefix in CASE_REFERENCE_PREFIXES {
        let reference = format!("{prefix}-2026/0042");
        assert!(CaseReference::parse(&reference).is_ok(), "{reference}");
    }
    for reference in [
        "CASE-1",
        "REQ-123456789012",
        "SEC-2026-11-0001",
        "TICKET-7/3",
    ] {
        assert!(CaseReference::parse(reference).is_ok(), "{reference}");
    }
    let too_long = format!("CASE-{}", "1".repeat(CASE_REFERENCE_MAX_LENGTH));
    for reference in [
        "",
        too_long.as_str(),
        "person@example.com",
        "dismissed for misconduct",
        "jane.doe",
        "JANE-DOE",
        "DOE-123",
        "CASE-DOE",
        "CASE-12a",
        "case-12",
        "CASE-",
        "CASE--12",
        "CASE-12/",
        "CASE-1-2-3-4",
        "HR#77",
        "ZD:12345",
        "CASE-Müller",
        "CASE-12\n",
        "-leading",
    ] {
        let mut value = valid();
        value["evidence_case_ref"] = json!(reference);
        assert_eq!(
            refused(&body(value)).1,
            "VALIDATION_FAILED",
            "{reference:?}"
        );
    }
    // A body above the bound is refused before it is parsed.
    let mut large = valid();
    large["evidence_case_ref"] = json!("A".repeat(REQUEST_BODY_LIMIT_BYTES));
    assert_eq!(
        refused(&body(large)),
        (StatusCode::PAYLOAD_TOO_LARGE, "BODY_TOO_LARGE".into())
    );
}

#[test]
fn case_reference_is_redacted_from_debug_output() {
    let parsed = OffboardRequest::parse(&body(valid())).unwrap();
    let printed = format!("{parsed:?}");
    assert!(!printed.contains("CASE-2026/0042"), "{printed}");
    assert!(printed.contains("REDACTED"));
}

/// The application grammar must stay inside the database checks on
/// `individual_offboardings`, so a valid request never fails on a constraint.
#[test]
fn contract_fits_the_history_table_checks() {
    for code in REASON_CODES {
        let bytes = code.as_bytes();
        assert!(bytes[0].is_ascii_uppercase());
        assert!((2..=48).contains(&bytes.len()));
        assert!(
            bytes
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
        );
    }
    const { assert!(CASE_REFERENCE_MAX_LENGTH <= 64) };
    for character in (0u8..=127).map(char::from) {
        let candidate = format!("CASE-1{character}2");
        if CaseReference::parse(&candidate).is_ok() {
            assert!(('!'..='~').contains(&character), "{character:?}");
        }
    }
    for prefix in CASE_REFERENCE_PREFIXES {
        assert!(prefix.bytes().all(|byte| byte.is_ascii_uppercase()));
    }
    assert_eq!(AUDIT_ACTION, "admin.user_offboarded");
    assert_eq!(PREVIEW_COUNT_LIMIT, 100);
    assert_eq!(OFFBOARD_ROUTE, "/api/auth/admin/users/{user_id}/offboard");
    assert_eq!(
        PREVIEW_ROUTE,
        "/api/auth/admin/users/{user_id}/offboarding-preview"
    );
}

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_sources(path: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(path).unwrap().flatten() {
        let child = entry.path();
        if child.is_dir() {
            rust_sources(&child, found);
        } else if child.extension().and_then(|value| value.to_str()) == Some("rs") {
            found.push(child);
        }
    }
}

/// No serving code implements the market hook, so no deployed configuration
/// can select a hook that skips the market work. The one implementation in
/// the crate forwards through `Arc` to a caller-supplied hook.
#[test]
fn no_serving_code_supplies_a_market_hook() {
    let mut sources = Vec::new();
    rust_sources(&source_root(), &mut sources);
    let mut implementations = Vec::new();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        let compact: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut cursor = 0;
        while let Some(found) = compact[cursor..].find("OffboardingMarketHook for ") {
            let start = cursor + found;
            let line: String = compact[start..].chars().take(60).collect();
            implementations.push(format!("{}: {line}", path.display()));
            cursor = start + 1;
        }
    }
    assert_eq!(implementations.len(), 1, "{implementations:#?}");
    assert!(
        implementations[0].contains("business/offboarding.rs: OffboardingMarketHook for Arc<H>"),
        "{implementations:#?}"
    );
}

/// The serving composition mounts neither route: `phase4::routes` never calls
/// the offboarding composition, and no other source names the router.
#[test]
fn serving_composition_leaves_offboarding_unmounted() {
    let phase4 = std::fs::read_to_string(source_root().join("runtime/phase4.rs")).unwrap();
    let start = phase4.find("pub fn routes(").unwrap();
    // The function ends at the first closing brace in column zero.
    let end = start + phase4[start..].find("\n}\n").unwrap();
    let serving = &phase4[start..end];
    assert!(
        !serving.contains("offboarding"),
        "phase4::routes mounts offboarding"
    );
    let mut sources = Vec::new();
    rust_sources(&source_root(), &mut sources);
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        let owner =
            path.ends_with("business/offboarding.rs") || path.ends_with("runtime/phase4.rs");
        if !owner {
            assert!(
                !text.contains("offboarding::router") && !text.contains("offboarding_routes"),
                "{} composes the offboarding routes",
                path.display()
            );
        }
    }
}

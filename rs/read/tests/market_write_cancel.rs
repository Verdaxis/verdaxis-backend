use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::routes::market_write::orders::{CancelInput, order_etag};

#[test]
fn cancel_reason_and_method_contract() {
    assert!(CancelInput::parse(false, None, None, None).is_ok());
    assert_eq!(
        CancelInput::parse(true, None, None, None)
            .unwrap_err()
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        CancelInput::parse(true, Some(json!({"reason":"  "})), None, None)
            .unwrap_err()
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        CancelInput::parse(true, Some(json!({"reason":" x "})), None, None)
            .unwrap()
            .reason
            .as_deref(),
        Some("x")
    );
    assert_eq!(
        CancelInput::parse(false, Some(json!({"reason":null})), None, None)
            .unwrap_err()
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[test]
fn cancel_etag_has_order_and_positive_version() {
    let id = Uuid::new_v4();
    let etag = order_etag(id, 2);
    let parsed = CancelInput::parse(false, None, Some(&etag), None).unwrap();
    assert_eq!(parsed.expected_version(id).unwrap(), Some(2));
    assert_eq!(
        parsed.expected_version(Uuid::new_v4()).unwrap_err().status,
        StatusCode::PRECONDITION_FAILED
    );
    for invalid in ["*", "W/\"2\"", "2", "\"order:invalid:2\""] {
        assert_eq!(
            CancelInput::parse(false, None, Some(invalid), None)
                .unwrap_err()
                .status,
            StatusCode::BAD_REQUEST
        );
    }
}

#[test]
fn cancel_key_is_normalized_and_bounded() {
    assert_eq!(
        CancelInput::parse(false, None, None, Some(" key "))
            .unwrap()
            .key
            .as_deref(),
        Some("key")
    );
    for invalid in ["".to_owned(), " ".to_owned(), "x".repeat(256)] {
        assert_eq!(
            CancelInput::parse(false, None, None, Some(&invalid))
                .unwrap_err()
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}

/// The status and the client-facing code of a refused cancellation input.
macro_rules! refusal_of {
    ($result:expr) => {{
        let error = $result.unwrap_err();
        (error.status, error.code.as_str())
    }};
}

#[test]
fn cancel_input_refusals_carry_their_codes() {
    assert_eq!(
        refusal_of!(CancelInput::parse(true, None, None, None)),
        (StatusCode::BAD_REQUEST, "VALIDATION_FAILED")
    );
    let short_reason = Some(json!({"reason": "x"}));
    assert_eq!(
        refusal_of!(CancelInput::parse(false, short_reason, None, None)),
        (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED")
    );
    assert_eq!(
        refusal_of!(CancelInput::parse(false, None, Some("*"), None)),
        (StatusCode::BAD_REQUEST, "IF_MATCH_MALFORMED")
    );
    assert_eq!(
        refusal_of!(CancelInput::parse(false, None, None, Some(" "))),
        (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED")
    );
    let etag = order_etag(Uuid::new_v4(), 3);
    let stale = CancelInput::parse(false, None, Some(&etag), None).unwrap();
    assert_eq!(
        refusal_of!(stale.expected_version(Uuid::new_v4())),
        (StatusCode::PRECONDITION_FAILED, "ORDER_VERSION_STALE")
    );
}

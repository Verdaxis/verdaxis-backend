use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::routes::market_write::orders::{CancelInput, order_etag};

#[test]
fn cancel_reason_and_method_contract() {
    assert!(CancelInput::parse(false, None, None, None).is_ok());
    // A POST without a reason is refused after the order lookup, not here.
    assert_eq!(CancelInput::parse(true, None, None, None).unwrap().reason, None);
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
    // A JSON null body is an absent optional body.
    assert_eq!(
        CancelInput::parse(false, Some(Value::Null), None, None)
            .unwrap()
            .reason,
        None
    );
}

#[test]
fn cancel_input_keeps_if_match_for_the_assisted_check() {
    let id = Uuid::new_v4();
    assert_eq!(order_etag(id, 2), format!("W/\"order-{id}-v2\""));
    // If-Match is compared only for an assisted order, after the order lookup.
    for raw in ["*", "W/\"2\"", "2", "", " "] {
        let parsed = CancelInput::parse(false, None, Some(raw), None).unwrap();
        assert_eq!(parsed.if_match.as_deref(), Some(raw));
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
            StatusCode::BAD_REQUEST
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
    let short_reason = Some(json!({"reason": "x"}));
    assert_eq!(
        refusal_of!(CancelInput::parse(false, short_reason, None, None)),
        (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED")
    );
    assert_eq!(
        refusal_of!(CancelInput::parse(false, None, None, Some(" "))),
        (StatusCode::BAD_REQUEST, "VALIDATION_FAILED")
    );
}

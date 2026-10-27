#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use verdaxis_read::routes::market_write::order_create::CreateInput;

fn now() -> DateTime<Utc> {
    "2026-10-27T12:00:00Z".parse().unwrap()
}
fn body() -> Value {
    json!({"side":"BID","product_id":"00000000-0000-0000-0000-000000000001","delivery_point_id":"00000000-0000-0000-0000-000000000002","quantity_mt":"200.00","price_per_mt_usd":"100.00"})
}

#[tokio::test]
async fn error_details_are_specific_and_generic_responses_are_unchanged() {
    use axum::response::IntoResponse;
    use verdaxis_read::{MarketErrorCode, MarketWriteError};
    async fn read(error: MarketWriteError) -> Value {
        let response = error.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    let kyc = read(MarketWriteError::kyc_required("REJECTED".into(), None)).await;
    assert_eq!(
        kyc["detail"],
        json!({"code":"KYC_REQUIRED","message":MarketErrorCode::KycRequired.message(),"kyc_status":"REJECTED","grace_expires_at":null})
    );
    for price in [None, Some(rust_decimal::Decimal::from(100))] {
        let crossing = read(MarketWriteError::post_only_would_cross(price)).await;
        assert_eq!(crossing["detail"].as_object().unwrap().len(), 3);
        assert_eq!(
            crossing["detail"]["best_executable_opposing_price"],
            json!(price.map(|p| p.to_string()))
        );
    }
    for code in MarketErrorCode::ALL {
        let error = MarketWriteError::new(*code);
        let response = error.into_response();
        if code.as_str().starts_with("MARKET_SUPPORT_CONTEXT_") {
            assert_eq!(
                response.headers()["x-verdaxis-market-support-context-invalid"],
                "true"
            );
            assert_eq!(
                response.headers()["x-verdaxis-market-support-context-code"],
                code.as_str()
            );
        }
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let actual: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            actual,
            json!({"detail":{"code":code.as_str(),"message":code.message()}})
        );
    }
    assert_eq!(
        MarketErrorCode::ResellerAllocationRequired
            .status()
            .as_u16(),
        422
    );
    assert_eq!(
        MarketErrorCode::SupportInstructionTimeInvalid
            .status()
            .as_u16(),
        400
    );
}

#[test]
fn create_schema_preserves_defaults_and_normalizes_certifications() {
    let mut value = body();
    value["certifications"] = json!([" A ", "", "A", "B", " B "]);
    let input = CreateInput::parse(value, now()).unwrap();
    let payload = input.economic_payload().unwrap();
    assert_eq!(payload["certifications"], json!(["A", "B"]));
    assert_eq!(payload["availability_window"], "SPOT");
    assert_eq!(payload["is_anonymous"], true);
    assert!(payload.get("support_confirmation").is_none());
    assert!(payload.get("fame_terms").is_none());
    assert!(payload.get("lng_terms").is_none());
    assert_eq!(payload["port_id"], Value::Null);
}

#[test]
fn create_schema_checks_each_economic_and_time_boundary() {
    for (field, invalid) in [
        ("quantity_mt", json!("0")),
        ("quantity_mt", json!("100000.01")),
        ("quantity_mt", json!("1.001")),
        ("quantity_mt", json!("NaN")),
        ("quantity_mt", Value::Null),
        ("price_per_mt_usd", json!("0")),
        ("price_per_mt_usd", json!("1000000.01")),
        ("price_per_mt_usd", json!("1.001")),
        ("expires_at", json!("2026-10-28T12:00:00")),
        ("is_anonymous", json!(false)),
        ("certifications", Value::Null),
    ] {
        let mut value = body();
        value[field] = invalid;
        assert_eq!(
            CreateInput::parse(value, now())
                .unwrap_err()
                .status
                .as_u16(),
            422,
            "{field}"
        );
    }
    let mut value = body();
    value["quantity_mt"] = json!("100000");
    value["price_per_mt_usd"] = json!("1000000");
    value["expires_at"] = json!("2026-10-28T12:00:00+08:00");
    assert!(CreateInput::parse(value, now()).is_ok());
}

#[test]
fn confirmation_uses_raw_reference_length_and_optional_trimmed_evidence() {
    let mut value = body();
    value["support_confirmation"] = json!({"external_instruction_reference":" x ","instruction_at":"2026-10-27T11:00:00Z","evidence_excerpt":"  ","acknowledge_exact_terms":true,"acknowledge_executable_standing_order":true});
    let input = CreateInput::parse(value.clone(), now()).unwrap();
    let confirmation = input.confirmation_payload().unwrap();
    assert_eq!(confirmation["external_instruction_reference"], "x");
    assert_eq!(confirmation["evidence_excerpt"], Value::Null);
    for (field, invalid) in [
        ("external_instruction_reference", json!("x")),
        ("external_instruction_reference", json!("   ")),
        ("instruction_at", json!("2026-10-27T12:00:01Z")),
        ("instruction_at", json!("2026-10-27T11:00:00")),
        ("acknowledge_exact_terms", json!(false)),
        ("acknowledge_executable_standing_order", json!(false)),
        ("evidence_excerpt", json!("a".repeat(4001))),
    ] {
        let mut invalid_value = value.clone();
        invalid_value["support_confirmation"][field] = invalid;
        assert!(CreateInput::parse(invalid_value, now()).is_err(), "{field}");
    }
}

#[test]
fn persisted_hashes_match_the_source_schema_payload() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut value = body();
    value["support_confirmation"] = json!({"external_instruction_reference":" x ","instruction_at":"2026-10-26T11:00:00Z","evidence_excerpt":"  ","acknowledge_exact_terms":true,"acknowledge_executable_standing_order":true});
    let input = CreateInput::parse(value.clone(), now()).unwrap();
    let script = "import json,sys; from app.schemas.orderbook import OrderCreate; from app.services.market_support import economic_order_idempotency_payload, authorization_terms_digest; from app.services.idempotency import idempotency_request_hash; o=OrderCreate.model_validate(json.load(sys.stdin)); p=economic_order_idempotency_payload(o); print(json.dumps([idempotency_request_hash(p),idempotency_request_hash({'order':p,'support_confirmation':o.support_confirmation.model_dump(mode='json')}),authorization_terms_digest(o)]))";
    let mut child =
        Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .env("ENVIRONMENT", "test")
            .env("RELEASE_SHA", "test")
            .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
            .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(value.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(input.request_hash(false).unwrap(), expected[0]);
    assert_eq!(input.request_hash(true).unwrap(), expected[1]);
    assert_eq!(input.authorization_digest().unwrap(), expected[2]);
}

#[test]
fn create_hash_tracks_normalized_economics_and_confirmation() {
    let mut left = body();
    left["certifications"] = json!([" A ", "A", ""]);
    let mut right = body();
    right["certifications"] = json!(["A"]);
    let left = CreateInput::parse(left, now()).unwrap();
    let right = CreateInput::parse(right, now()).unwrap();
    assert_eq!(
        left.request_hash(false).unwrap(),
        right.request_hash(false).unwrap()
    );
    let mut changed = body();
    changed["quantity_mt"] = json!("201.00");
    let changed = CreateInput::parse(changed, now()).unwrap();
    assert_ne!(
        left.request_hash(false).unwrap(),
        changed.request_hash(false).unwrap()
    );
}

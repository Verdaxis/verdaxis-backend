use serde_json::json;
use verdaxis_read::MarketErrorCode;
use verdaxis_read::commission::{CommissionPatch, CommissionStatus, PatchField};

#[test]
fn commission_patch_preserves_omitted_and_clearable_fields() {
    let patch =
        CommissionPatch::parse(json!({"invoice_number":null,"notes":"","amount_usd":"1"})).unwrap();
    assert_eq!(patch.status, None);
    assert_eq!(patch.invoice_number, PatchField::Set(None));
    assert_eq!(patch.invoice_date, PatchField::Keep);
    assert_eq!(patch.payment_date, PatchField::Keep);
    assert_eq!(patch.notes, PatchField::Set(Some(String::new())));
}

#[test]
fn commission_status_accepts_each_stored_value() {
    for (text, status) in [
        ("PENDING", CommissionStatus::Pending),
        ("INVOICED", CommissionStatus::Invoiced),
        ("PAID", CommissionStatus::Paid),
    ] {
        assert_eq!(
            CommissionPatch::parse(json!({"status":text}))
                .unwrap()
                .status,
            Some(status)
        );
    }
    for payload in [
        json!({"status":"OTHER"}),
        json!({"invoice_date":"2026-02-30"}),
        json!({"notes":42}),
        json!([]),
    ] {
        assert!(CommissionPatch::parse(payload).is_err());
    }
}

#[test]
fn commission_patch_limits_text_by_character_count() {
    let longest = json!({"invoice_number":"é".repeat(50),"notes":"界".repeat(500)});
    let patch = CommissionPatch::parse(longest).unwrap();
    assert_eq!(patch.invoice_number, PatchField::Set(Some("é".repeat(50))));
    assert_eq!(patch.notes, PatchField::Set(Some("界".repeat(500))));
    for payload in [
        json!({"invoice_number":"é".repeat(51)}),
        json!({"notes":"界".repeat(501)}),
    ] {
        let error = CommissionPatch::parse(payload).unwrap_err();
        assert_eq!(error.code, MarketErrorCode::ValidationFailed);
    }
}

#[test]
fn commission_module_writes_only_through_the_effect_port() {
    let source = include_str!("../src/order_write/commission.rs");
    assert!(!source.contains("FROM trades"));
    assert!(!source.contains("INSERT INTO commissions"));
    assert!(!source.contains("INSERT INTO audit_logs"));
    assert!(!source.contains("INSERT INTO notifications"));
    assert!(!source.contains("INSERT INTO market_event_outbox"));
}

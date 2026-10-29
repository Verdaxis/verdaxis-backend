use serde_json::json;
use verdaxis_read::order_write::freeze::{FreezeRequest, FrozenPage, UnfreezeRequest};

#[test]
fn reasons_and_trimmed_character_bounds_are_validated() {
    for reason in [
        "PAYMENT_DISPUTE",
        "DELIVERY_DISPUTE",
        "TERMS_DISPUTE",
        "COMPLIANCE_REVIEW",
        "FRAUD_REVIEW",
        "COURT_ORDER",
        "OTHER",
    ] {
        let parsed =
            FreezeRequest::parse(&json!({"reason_code":reason,"note":"\u{2003}\t abc \n"}))
                .unwrap();
        assert_eq!(parsed.note, "abc");
        assert_eq!(serde_json::to_value(parsed.reason_code).unwrap(), reason);
    }
    for note in ["ab".to_owned(), " \t\n".to_owned(), "é".repeat(501)] {
        assert!(UnfreezeRequest::parse(&json!({"note":note})).is_err());
    }
    for note in ["é".repeat(3), "é".repeat(500)] {
        assert_eq!(
            UnfreezeRequest::parse(&json!({"note":note})).unwrap().note,
            note
        );
    }
    for value in [
        json!({"reason_code":"UNKNOWN","note":"valid"}),
        json!({"reason_code":"OTHER","note":null}),
        json!({"note":"valid"}),
        json!([]),
    ] {
        assert!(FreezeRequest::parse(&value).is_err());
    }
}

#[test]
fn active_page_has_nonnegative_offset_and_bounded_limit() {
    let page = FrozenPage::parse(None).unwrap();
    assert_eq!((page.skip, page.limit), (0, 25));
    for query in [
        "skip=-1",
        "limit=0",
        "limit=101",
        "skip=x",
        "limit=1.5",
        "skip=9223372036854775808",
    ] {
        assert!(FrozenPage::parse(Some(query)).is_err(), "{query}");
    }
    for query in ["skip=0&limit=1", "skip=9223372036854775807&limit=100"] {
        assert!(FrozenPage::parse(Some(query)).is_ok());
    }
}

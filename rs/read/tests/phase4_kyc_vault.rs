use uuid::Uuid;
use verdaxis_read::kyc::{
    keyring::KeyRing,
    vault::{self, DocumentKind, DocumentMetadata},
};

fn keys() -> KeyRing {
    KeyRing::parse(br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#)
        .expect("synthetic key ring")
}

fn metadata() -> DocumentMetadata {
    DocumentMetadata {
        id: Uuid::new_v4(),
        submission_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        kind: DocumentKind::Identity,
        content_type: "application/pdf".into(),
    }
}

#[test]
fn p4_t10_c01() {
    for raw in [
        br#"{"active":"a","active":"a","keys":{}}"#.as_slice(),
        br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#,
        br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB="}}"#,
        br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="},"extra":1}"#,
        br#"{"active":"missing","keys":{}}"#,
    ] { assert!(KeyRing::parse(raw).is_err(), "key validation must refuse"); }
    assert!(KeyRing::parse(&vec![b' '; 1024 * 1024 + 1]).is_err());
}

#[test]
fn p4_t10_c02() {
    let keys = keys();
    let mut row = vault::encrypt(&keys, metadata(), b"%PDF-synthetic").expect("encrypt");
    assert!(vault::decrypt(&keys, &row).is_ok());
    row.metadata.organization_id = Uuid::new_v4();
    assert!(vault::decrypt(&keys, &row).is_err());
    let mut row = vault::encrypt(&keys, metadata(), b"%PDF-synthetic").expect("encrypt");
    row.ciphertext[0] ^= 1;
    assert!(vault::decrypt(&keys, &row).is_err());
    let mut row = vault::encrypt(&keys, metadata(), b"%PDF-synthetic").expect("encrypt");
    row.key_id = "unavailable".into();
    assert!(vault::decrypt(&keys, &row).is_err());
    let mut row = vault::encrypt(&keys, metadata(), b"%PDF-synthetic").expect("encrypt");
    row.size_bytes += 1;
    assert!(vault::decrypt(&keys, &row).is_err());
}

#[test]
fn p4_t10_c06() {
    let old = keys();
    let rotated = KeyRing::parse(br#"{"active":"b","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","b":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="}}"#).expect("rotation");
    let row = vault::encrypt(&old, metadata(), b"%PDF-synthetic").expect("encrypt");
    let content = vault::decrypt(&rotated, &row).expect("retained key");
    assert!(
        content.as_slice() == b"%PDF-synthetic",
        "retained content must round trip"
    );
    let new = vault::encrypt(&rotated, metadata(), b"%PDF-synthetic").expect("encrypt");
    assert_eq!(new.key_id, "b");
    assert_ne!(new.data_nonce, new.key_nonce);
}

#[test]
fn p4_t10_c07() {
    let keys = keys();
    assert_eq!(format!("{keys:?}"), "KeyRing([REDACTED])");
    let row = vault::encrypt(&keys, metadata(), b"%PDF-synthetic").expect("encrypt");
    assert_eq!(format!("{row:?}"), "EncryptedDocument([REDACTED])");
}

#[test]
fn p4_t10_c18() {
    assert!(KeyRing::load_runtime(None, None, "production").is_err());
    assert!(
        KeyRing::load_runtime(None, Some(std::path::Path::new("/nonexistent")), "test").is_err()
    );
    assert!(KeyRing::load_runtime(
        None,
        Some(std::path::Path::new("/nonexistent")),
        "production"
    )
    .is_err());
    assert_eq!(
        format!("{:?}", KeyRing::parse(b"invalid").unwrap_err()),
        "VaultError"
    );
}

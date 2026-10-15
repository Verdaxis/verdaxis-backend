use verdaxis_read::{compiled_sha, verify_release};

#[test]
fn binary_identity_cannot_be_overridden() {
    assert_eq!(compiled_sha().len(), 40);
    assert!(verify_release(compiled_sha()).is_ok());
    let other = if compiled_sha().starts_with('a') {
        "b"
    } else {
        "a"
    }
    .repeat(40);
    assert!(verify_release(&other).is_err());
    assert!(verify_release("test").is_err());
}

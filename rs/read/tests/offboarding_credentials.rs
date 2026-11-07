//! Offline checks of the account credential cutoff primitive.
//!
//! `python3`) on the unchanged Python password verifier.

use verdaxis_read::auth::{
    offboarding::{CUTOFF_SKEW_MARGIN_SECONDS, UnusablePasswordHash},
    password,
};

#[test]
fn skew_margin_is_a_few_seconds() {
    assert!(
        (1..=10).contains(&CUTOFF_SKEW_MARGIN_SECONDS),
        "the documented margin covers host clock differences without a long lockout window"
    );
}

#[tokio::test]
async fn unusable_hash_rejected_by_both_stacks() {
    let first = UnusablePasswordHash::prepare()
        .await
        .expect("hash")
        .as_str()
        .to_owned();
    let second = UnusablePasswordHash::prepare()
        .await
        .expect("hash")
        .as_str()
        .to_owned();
    assert_ne!(first, second, "every account gets its own unusable hash");
    for hash in [&first, &second] {
        assert!(
            hash.starts_with(&format!("$2b${}$", password::BCRYPT_COST)),
            "a standard bcrypt hash both stacks parse, at the real password cost"
        );
        assert_eq!(hash.len(), 60);
    }
    let long = "x".repeat(200);
    let candidates = [
        "",
        "password123",
        "SyntheticPassword123",
        first.as_str(),
        second.as_str(),
        long.as_str(),
    ];
    for candidate in candidates {
        assert!(
            !password::verify(candidate, &first).await,
            "the Rust verifier must reject every presented password"
        );
    }
}

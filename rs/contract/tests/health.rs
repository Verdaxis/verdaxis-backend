//! Health group parity against the recorded Python corpus.
//!
//! The disposable runner supplies both endpoint URLs and the token.

#[test]
fn health_contract_matches_python() {
    verdaxis_contract::compare_group("health").expect("health contract group");
}

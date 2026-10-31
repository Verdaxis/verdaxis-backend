//! Task 18 allocation vectors. PostgreSQL schedules are supplied by the runner.
use verdaxis_read::crud::referrals::{COLLISION_PROBES, Candidates, referral_code_response};

#[test]
fn allocation_has_exactly_ten_candidate_probes() {
    assert_eq!(COLLISION_PROBES, 10);
    let mut candidates = Candidates::fixed(vec!["VDX-AAAAAA".into(), "VDX-BBBBBB".into()]);
    assert_eq!(candidates.next().as_deref(), Some("VDX-AAAAAA"));
    assert_eq!(candidates.next().as_deref(), Some("VDX-BBBBBB"));
    assert_eq!(candidates.next(), None);
}

#[test]
fn code_response_keeps_frontend_trailing_slash() {
    for (base, expected) in [
        (
            "https://app.example",
            "https://app.example/invite/VDX-ABC123",
        ),
        (
            "https://app.example/",
            "https://app.example//invite/VDX-ABC123",
        ),
    ] {
        let response = referral_code_response(base, "VDX-ABC123");
        assert_eq!(response["referral_code"], "VDX-ABC123");
        assert_eq!(response["referral_link"], expected);
    }
}

#[test]
fn random_candidates_use_the_public_code_grammar() {
    let mut candidates = Candidates::default();
    for _ in 0..32 {
        let code = candidates.next().unwrap();
        assert!(verdaxis_read::crud::referrals::code_matches_python_format(
            &code
        ));
        assert_eq!(code.len(), 10);
    }
}

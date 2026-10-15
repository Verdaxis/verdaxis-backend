//! Task 4 self-test: the echo projection exercises the whole pipeline.

#[test]
fn echo_contract_matches_python() {
    verdaxis_contract::compare_group("echo").expect("echo contract group");
}

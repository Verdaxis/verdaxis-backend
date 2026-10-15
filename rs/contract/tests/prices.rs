//! Task 9 HTTP parity. External processes are owned by the disposable runner.
#[cfg(feature = "contract-http")]
#[test]
fn prices_and_tape_match_python() {
    let parity = verdaxis_contract::compare_group("prices");
    let status = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../venv/bin/python"
    ))
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/prices_oracle.py"
    ))
    .status()
    .expect("run price fixture comparison");
    assert!(status.success(), "populated price comparison failed");
    parity.expect("prices and trade tape contract");
}

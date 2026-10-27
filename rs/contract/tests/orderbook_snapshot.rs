//! R15 uses only the shared runner's attested disposable servers.
#[cfg(feature = "contract-http")]
#[test]
fn snapshot_contract_matches() {
    verdaxis_contract::compare_group("orderbook_snapshot").unwrap();
    let status = std::process::Command::new(std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()))
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/orderbook_snapshot_oracle.py"
    ))
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run snapshot edge comparisons");
    assert!(status.success(), "snapshot edge comparison failed");
}

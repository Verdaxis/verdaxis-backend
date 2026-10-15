//! Task 7c compares the populated, attested disposable corpus.
#[cfg(feature = "contract-http")]
#[test]
fn map_and_compatibility_match_python() {
    verdaxis_contract::compare_group("orderbook_map").unwrap();
    let status = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../venv/bin/python"
    ))
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/orderbook_map_oracle.py"
    ))
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run map and compatibility edge comparisons");
    assert!(
        status.success(),
        "map and compatibility edge comparison failed"
    );
}

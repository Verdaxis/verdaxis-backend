//! R16–R18/R26 use the runner's attested disposable servers.
#[cfg(feature = "contract-http")]
#[test]
fn ask_offset_past_end_keeps_total() {
    verdaxis_contract::compare_group("orderbook_lists").unwrap();
    let status = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../venv/bin/python"
    ))
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/orderbook_lists_oracle.py"
    ))
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run list edge comparisons");
    assert!(status.success(), "list edge comparison failed");
}

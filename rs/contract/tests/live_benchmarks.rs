//! Task 6a helper-only HTTP parity. No public endpoint is added.
#[cfg(feature = "contract-http")]
#[test]
fn live_benchmarks_match_python() {
    verdaxis_contract::compare_group("live_benchmarks").expect("live benchmark parity");
    let status = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../venv/bin/python"
    ))
    .args(["-m", "tests.rust_contract.live_benchmarks_probe"])
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run migrated fixture comparison");
    assert!(status.success(), "live benchmark matrix differs");
}

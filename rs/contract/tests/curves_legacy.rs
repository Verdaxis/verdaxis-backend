//! R07/R08 parity runs only against the runner's attested disposable servers.
#[cfg(feature = "contract-http")]
#[test]
fn legacy_curve_contracts_match() {
    verdaxis_contract::compare_group("curves_legacy").unwrap();
    let status = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../venv/bin/python"
    ))
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/legacy_curve_oracle.py"
    ))
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run legacy relation comparison");
    assert!(status.success(), "legacy relation differs from Python");
}

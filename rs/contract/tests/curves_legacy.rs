//! R07/R08 parity runs only against the runner's attested disposable servers.
#[cfg(feature = "contract-http")]
#[test]
fn legacy_curve_contracts_match() {
    verdaxis_contract::compare_group("curves_legacy").unwrap();
    let status = std::process::Command::new(
        std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()),
    )
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/legacy_curve_oracle.py"
    ))
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run legacy relation comparison");
    assert!(status.success(), "legacy relation differs from Python");
}

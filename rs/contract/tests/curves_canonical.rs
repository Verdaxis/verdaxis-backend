//! Canonical curve gates use only the harness-attested disposable services.
#[cfg(feature = "contract-http")]
#[test]
fn all_curve_shapes_match_python() {
    verdaxis_contract::compare_group("curves_canonical").unwrap();
    let status = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../venv/bin/python"
    ))
    .arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../read/tests/curves_canonical_oracle.py"
    ))
    .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    .status()
    .expect("run canonical curve oracle");
    assert!(status.success(), "canonical curve acceptance probes failed");
}

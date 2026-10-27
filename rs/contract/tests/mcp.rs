#![cfg(feature = "contract-http")]

#[test]
fn mcp_matches_python_and_saved_lane_evidence() {
    verdaxis_contract::compare_group("mcp").unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new(
        std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()),
    )
    .current_dir(&root)
    .args(["-m", "scripts.rust_contract.mcp_acceptance"])
    .status()
    .unwrap();
    assert!(
        status.success(),
        "MCP privacy or SQL cancellation proof failed"
    );
    if let Ok(evidence) = std::env::var("MCP_LANE_EVIDENCE") {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let gate = if std::env::var("KYC_EXECUTION_GATE_ENABLED").unwrap() == "true" {
            "enabled"
        } else {
            "disabled"
        };
        let status = std::process::Command::new(
            std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into()),
        )
        .current_dir(&root)
        .args([
            "-m",
            "scripts.rust_contract.replay_mcp",
            "--base-url",
            &std::env::var("RUST_CONTRACT_URL").unwrap(),
            "--evidence",
            &evidence,
            "--kyc-gate",
            gate,
        ])
        .status()
        .unwrap();
        assert!(status.success(), "saved MCP evidence differs");
    }
}

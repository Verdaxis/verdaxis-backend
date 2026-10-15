#![cfg(feature = "contract-http")]

#[test]
fn mcp_matches_python_and_saved_lane_evidence() {
    verdaxis_contract::compare_group("mcp").unwrap();
    if let Ok(evidence) = std::env::var("MCP_LANE_EVIDENCE") {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let gate = if std::env::var("KYC_EXECUTION_GATE_ENABLED").unwrap() == "true" {
            "enabled"
        } else {
            "disabled"
        };
        let status = std::process::Command::new(root.join("venv/bin/python"))
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

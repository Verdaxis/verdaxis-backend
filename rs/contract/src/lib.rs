//! Independent contract test and CLI package for the Rust read port.
//!
//! It drives external HTTP processes and delegates every comparison to the one
//! authoritative Python comparator. It never links the read package and holds
//! no second comparator or normalization implementation.

mod frozen_gate;
mod gate;
pub use frozen_gate::{
    GateOutcome, PerformanceVerdict, RustMeasurement, RustRoute, frozen_performance_gate,
    meaning_performance_gate,
};

pub use gate::{
    ContractEvidence, RouteP95, RouteRequests, WorkloadSize, blocking_regressions,
    contract_differences, expected_route_ids, fix_item_regressions, passes_gate,
    passes_paired_gate, same_workload,
};

/// Run the external comparator for one contract group.
///
/// The comparator is started with the explicit `PYTHON_CONTRACT_URL`,
/// `RUST_CONTRACT_URL`, and `DISPOSABLE_TEST_TOKEN` already in the
/// environment. A missing input, failed handshake, unknown group, or any
/// mismatch is an error.
pub fn compare_group(group: &str) -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new(root.join("venv/bin/python"))
        .current_dir(&root)
        .args(["-m", "scripts.rust_contract.compare", "--group", group])
        .status()
        .map_err(|_| "comparator could not start".to_owned())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("contract group failed: {group}"))
    }
}

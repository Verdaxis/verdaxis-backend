//! Successor scope does not change the first byte-cutover performance gate.
use verdaxis_contract::{
    GateOutcome, RustMeasurement, RustRoute, WorkloadSize, meaning_performance_gate,
};

fn run() -> RustMeasurement {
    let base: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/python-baseline-small.json"
    ))
    .unwrap();
    RustMeasurement {
        size: WorkloadSize::Large,
        source_sha: "a".repeat(40),
        harness_sha: "a".repeat(40),
        manifest_digest: base["manifest_digest"].as_str().unwrap().to_owned(),
        baseline_sha256: "3c74f7960add211757198149241f1633ef90a165ec89a902ac40bf79c1f8ea5e"
            .to_owned(),
        measurement_date: "2026-10-18".to_owned(),
        offered_rates: vec![2.0],
        routes: base["routes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, row)| RustRoute {
                route_id: id.clone(),
                p95: row["p95"].as_f64().unwrap() / 2.0,
                offered: 1000,
                successful: 1000,
                errors_5xx: 0,
            })
            .collect(),
    }
}

fn decide(run: &RustMeasurement) -> GateOutcome {
    meaning_performance_gate(
        include_bytes!("../../../tests/rust_contract/python-baseline-small.json"),
        include_str!("../../../tests/rust_contract/gate-config.json"),
        run,
    )
    .outcome
}

#[test]
fn successor_accepts_a_lower_declared_rate_without_new_python_samples() {
    assert_eq!(decide(&run()), GateOutcome::Pass);
}

#[test]
fn large_errors_block_but_large_speed_is_a_fix_item() {
    let mut run = run();
    for row in &mut run.routes {
        row.p95 = 5.0;
    }
    assert_eq!(decide(&run), GateOutcome::Pass);
    for row in &mut run.routes {
        row.offered = 2000;
        row.errors_5xx = 1000;
    }
    assert_eq!(decide(&run), GateOutcome::NoGo);
}

#[test]
fn successor_keeps_full_sample_floor() {
    let mut run = run();
    run.routes[0].successful = 999;
    assert_eq!(decide(&run), GateOutcome::Inconclusive);
}

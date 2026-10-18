//! Decisions 86/87 share baseline bytes and pin with the Python report.
use verdaxis_contract::{
    GateOutcome, RustMeasurement, RustRoute, WorkloadSize, frozen_performance_gate,
};

fn measurement() -> RustMeasurement {
    let baseline: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/python-baseline-small.json"
    ))
    .unwrap();
    let config: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/gate-config.json"
    ))
    .unwrap();
    RustMeasurement {
        size: WorkloadSize::Small,
        source_sha: "a".repeat(40),
        harness_sha: "a".repeat(40),
        manifest_digest: baseline["manifest_digest"].as_str().unwrap().to_owned(),
        baseline_sha256: config["python_baseline_sha256"]
            .as_str()
            .unwrap()
            .to_owned(),
        measurement_date: "2026-10-18".to_owned(),
        offered_rates: vec![10.0],
        routes: baseline["routes"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, row)| RustRoute {
                route_id: id.to_owned(),
                p95: row["p95"].as_f64().unwrap() / 2.0,
                offered: 1000,
                successful: 1000,
                errors_5xx: 0,
            })
            .collect(),
    }
}

fn decide(run: &RustMeasurement) -> GateOutcome {
    frozen_performance_gate(
        include_bytes!("../../../tests/rust_contract/python-baseline-small.json"),
        include_str!("../../../tests/rust_contract/gate-config.json"),
        run,
    )
    .outcome
}

#[test]
fn frozen_rust_within_limits_passes() {
    assert_eq!(decide(&measurement()), GateOutcome::Pass);
}

#[test]
fn changed_or_missing_baseline_pin_is_inconclusive() {
    let run = measurement();
    for bytes in [b"{}".as_slice(), b"".as_slice()] {
        assert_eq!(
            frozen_performance_gate(
                bytes,
                include_str!("../../../tests/rust_contract/gate-config.json"),
                &run
            )
            .outcome,
            GateOutcome::Inconclusive
        );
    }
    assert_eq!(
        frozen_performance_gate(
            include_bytes!("../../../tests/rust_contract/python-baseline-small.json"),
            "{}",
            &run
        )
        .outcome,
        GateOutcome::Inconclusive
    );
}

#[test]
fn missing_routes_floor_or_identity_are_inconclusive() {
    for defect in 0..4 {
        let mut run = measurement();
        match defect {
            0 => {
                run.routes.pop();
            }
            1 => run.routes[0].successful = 999,
            2 => run.harness_sha.clear(),
            _ => run.baseline_sha256.clear(),
        }
        assert_eq!(decide(&run), GateOutcome::Inconclusive);
    }
}

#[test]
fn frozen_thresholds_and_error_rate_block_small_only() {
    let mut run = measurement();
    run.routes[0].p95 = 5.0;
    assert_eq!(decide(&run), GateOutcome::NoGo);
    run.size = WorkloadSize::Large;
    let result = frozen_performance_gate(
        include_bytes!("../../../tests/rust_contract/python-baseline-small.json"),
        include_str!("../../../tests/rust_contract/gate-config.json"),
        &run,
    );
    assert_eq!(result.outcome, GateOutcome::Pass);
    assert!(result.fix_items.contains(&run.routes[0].route_id));
    run = measurement();
    run.routes[0].errors_5xx = 2000;
    run.routes[0].offered += 2000;
    assert_eq!(decide(&run), GateOutcome::NoGo);
}

#[test]
fn baseline_missing_route_and_lowered_floor_are_inconclusive() {
    use sha2::{Digest, Sha256};
    let mut base: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../tests/rust_contract/python-baseline-small.json"
    ))
    .unwrap();
    base["routes"].as_object_mut().unwrap().remove("R27");
    let bytes = serde_json::to_vec(&base).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/gate-config.json"
    ))
    .unwrap();
    config["python_baseline_sha256"] = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .into();
    let mut run = measurement();
    run.baseline_sha256 = config["python_baseline_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        frozen_performance_gate(&bytes, &config.to_string(), &run).outcome,
        GateOutcome::Inconclusive
    );
    config = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/gate-config.json"
    ))
    .unwrap();
    config["rust_successful_samples_per_route"] = 0.into();
    assert_eq!(
        frozen_performance_gate(
            include_bytes!("../../../tests/rust_contract/python-baseline-small.json"),
            &config.to_string(),
            &measurement()
        )
        .outcome,
        GateOutcome::Inconclusive
    );
}

#[test]
fn frozen_route_just_inside_and_outside_relative_limit() {
    let base: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../tests/rust_contract/python-baseline-small.json"
    ))
    .unwrap();
    let mut run = measurement();
    let row = run.routes.iter_mut().find(|r| r.route_id == "R27").unwrap();
    let python_us = (base["routes"]["R27"]["p95"].as_f64().unwrap() * 1_000_000.0).round() as u64;
    row.p95 = (python_us + python_us / 2) as f64 / 1_000_000.0;
    assert_eq!(decide(&run), GateOutcome::Pass);
    run.routes
        .iter_mut()
        .find(|r| r.route_id == "R27")
        .unwrap()
        .p95 += 0.000001;
    assert_eq!(decide(&run), GateOutcome::NoGo);
}

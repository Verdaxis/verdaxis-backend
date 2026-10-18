//! False-pass tests for the decision-68 go/no-go gate.
//!
//! Every test here is written to fail if the gate ever passes on a condition
//! that is missing, unmeasured, or spelled as a placeholder.

use verdaxis_contract::{ContractEvidence, RouteP95, RouteRequests};
use verdaxis_contract::{blocking_regressions, fix_item_regressions, passes_gate};

fn route(id: &'static str, python: f64, rust: f64) -> RouteP95 {
    RouteP95 {
        route_id: id,
        python,
        rust,
    }
}

fn complete_routes() -> Vec<RouteP95> {
    static MANIFEST: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let manifest = MANIFEST.get_or_init(|| serde_json::from_str(include_str!("../../../tests/rust_contract/workload.json")).unwrap());
    ["routes", "tools"].iter().flat_map(|key| manifest[key].as_array().unwrap()).map(|row| {
        route(row["id"].as_str().unwrap(), 0.2, 0.1)
    }).collect()
}

fn complete_contract(differences: u64) -> Vec<ContractEvidence> {
    static CASES: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let cases = CASES.get_or_init(|| serde_json::from_str(include_str!("../../../tests/rust_contract/cases.json")).unwrap());
    let mut rows: Vec<_> = ["small", "large"].iter().flat_map(|size| {
        cases["groups"].as_object().unwrap().keys().map(move |group| ContractEvidence {
            size, group, cases: 1, differences: Some(0),
        })
    }).collect();
    rows[0].differences = Some(differences);
    rows
}

fn complete_requests() -> Vec<RouteRequests> {
    complete_routes().iter().map(|row| RouteRequests { route_id: row.route_id, python: 1000, rust: 1000 }).collect()
}

#[allow(clippy::too_many_arguments)]
fn gate_with_routes(differences: u64, rust_errors: u64, python_errors: u64, integration: bool, rust_mix: f64, python_mix: f64, overrides: &[RouteP95]) -> bool {
    let mut routes = complete_routes();
    for item in overrides {
        if let Some(existing) = routes.iter_mut().find(|r| r.route_id == item.route_id) {
            *existing = *item;
        } else {
            routes.push(*item);
        }
    }
    passes_gate(&complete_contract(differences), rust_errors, python_errors, integration, rust_mix, python_mix, &routes, &complete_requests())
}

#[test]
fn all_hard_gates_and_speed_pass() {
    let routes = [route("R01", 0.2, 0.1), route("R02", 0.2, 0.3)];
    assert!(gate_with_routes(0, 3, 5, true, 0.5, 0.6, &routes));
    // Equal 5xx counts are not a rise.
    assert!(gate_with_routes(0, 5, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn one_unexplained_difference_blocks() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!gate_with_routes(1, 0, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn a_5xx_rise_blocks_even_with_everything_else_passing() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!gate_with_routes(0, 6, 5, true, 0.5, 0.6, &routes));
    // One more Rust 503 than Python's, with zero differences, a passing
    // integration suite and a faster mix, is still a no-go.
    assert!(!gate_with_routes(0, 1, 0, true, 0.1, 10.0, &routes));
}

#[test]
fn a_failing_integration_suite_blocks() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!gate_with_routes(0, 0, 5, false, 0.5, 0.6, &routes));
}

#[test]
fn a_mix_p95_regression_blocks() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!gate_with_routes(0, 0, 5, true, 0.7, 0.6, &routes));
}

// ------------------------------------------------- per-route speed thresholds

#[test]
fn a_regression_of_exactly_100_ms_does_not_block() {
    // 0.200 s -> 0.300 s is exactly the 100 ms / 50 % boundary: not "more
    // than" either threshold, so the gate still passes and the route is a fix
    // item, not a block.
    assert!(gate_with_routes(
        0,
        0,
        5,
        true,
        0.5,
        0.6,
        &[route("R20", 0.200, 0.300)]
    ));
    assert_eq!(fix_item_regressions(&[route("R20", 0.200, 0.300)]), ["R20"]);
    assert!(blocking_regressions(&[route("R20", 0.200, 0.300)]).is_empty());
}

#[test]
fn one_millisecond_above_the_boundary_blocks() {
    // 0.200 s -> 0.301 s crosses the 100 ms absolute threshold, so the route
    // blocks even with every hard gate passing.
    assert!(!gate_with_routes(
        0,
        0,
        5,
        true,
        0.5,
        0.6,
        &[route("R20", 0.200, 0.301)]
    ));
}

#[test]
fn a_regression_of_exactly_50_percent_does_not_block_when_under_100_ms() {
    // 0.100 s -> 0.150 s is exactly 50 % and 50 ms: under the absolute bar and
    // not "more than" 50 %, so it is a fix item.
    assert!(gate_with_routes(
        0,
        0,
        5,
        true,
        0.5,
        0.6,
        &[route("R01", 0.100, 0.150)]
    ));
    assert_eq!(
        fix_item_regressions(&[route("R01", 0.100, 0.150)]),
        ["R01"]
    );
}

#[test]
fn a_route_that_crosses_only_the_relative_threshold_blocks() {
    let routes = [route("R01", 0.100, 0.160)];
    assert_eq!(blocking_regressions(&routes), ["R01"]);
    assert!(!gate_with_routes(0, 0, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn a_route_that_crosses_only_the_absolute_threshold_blocks() {
    // 0.120 s delta over a slow 1.0 s Python p95: 12 % relative, but over the
    // 100 ms absolute bar.
    let routes = [route("R02", 1.000, 1.120)];
    assert_eq!(blocking_regressions(&routes), ["R02"]);
}

#[test]
fn smaller_regressions_do_not_block_and_become_fix_items() {
    let routes = [
        route("R20", 0.286, 0.327), // +41 ms, +14 %: a fix item
        route("R24", 0.214, 0.236), // +22 ms, +10 %: a fix item
        route("R01", 0.569, 0.085), // an improvement
    ];
    assert!(blocking_regressions(&routes).is_empty());
    assert_eq!(fix_item_regressions(&routes), ["R20", "R24"]);
    assert!(gate_with_routes(0, 0, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn a_mix_regression_with_every_route_inside_its_limit_still_blocks() {
    // Every route is inside its own threshold, but the weighted mix is worse,
    // so the mix condition blocks.
    let routes = [route("R01", 0.100, 0.150), route("R02", 0.100, 0.150)];
    assert!(blocking_regressions(&routes).is_empty());
    assert!(!gate_with_routes(0, 0, 5, true, 0.55, 0.50, &routes));
}

#[test]
fn an_improvement_never_blocks() {
    let routes = [route("R16", 2.806, 1.460)];
    assert!(blocking_regressions(&routes).is_empty());
    assert!(fix_item_regressions(&routes).is_empty());
}

#[test]
fn unmeasured_or_nonfinite_latency_never_passes() {
    // A placeholder p95 of zero on the Python side is not a measurement: the
    // mix comparison requires a positive, finite Python mix p95.
    assert!(!gate_with_routes(
        0,
        0,
        0,
        true,
        0.0,
        0.0,
        &[route("R01", 0.0, 0.0)]
    ));
    assert!(!gate_with_routes(
        0,
        0,
        0,
        true,
        f64::NAN,
        0.5,
        &[route("R01", 0.2, 0.1)]
    ));
    assert!(!gate_with_routes(
        0,
        0,
        0,
        true,
        0.5,
        f64::INFINITY,
        &[route("R01", 0.2, 0.1)]
    ));
}

#[test]
fn either_mix_p95_must_be_finite_and_positive() {
    for invalid in [0.0, -0.1, f64::NAN, f64::INFINITY] {
        let routes = [route("R01", 0.2, 0.1)];
        assert!(!gate_with_routes(0, 0, 0, true, invalid, 0.6, &routes));
        assert!(!gate_with_routes(0, 0, 0, true, 0.5, invalid, &routes));
    }
}

#[test]
fn a_zero_python_mix_p95_is_not_a_measurement() {
    // Rust 0.0 against Python 0.0: without a positive Python baseline the mix
    // comparison is vacuous, so the gate must refuse it.
    assert!(!gate_with_routes(0, 0, 0, true, 0.0, 0.0, &[]));
}

#[test]
fn route_inputs_must_be_finite_before_the_gate() {
    // NaN comparisons are false, so a NaN pair would never "block"; the gate
    // refuses non-finite route input instead of trusting the comparison.
    let nan_route = route("R01", f64::NAN, f64::NAN);
    assert!(!(nan_route.python.is_finite() && nan_route.rust.is_finite()));
    assert!(!gate_with_routes(0, 0, 0, true, 0.5, 0.6, &[nan_route]));
}

#[test]
fn missing_or_empty_manifest_routes_never_pass() {
    assert!(!passes_gate(&complete_contract(0), 0, 0, true, 0.5, 0.6, &[], &complete_requests()));
    let mut routes = complete_routes();
    routes.pop();
    assert!(!passes_gate(&complete_contract(0), 0, 0, true, 0.5, 0.6, &routes, &complete_requests()));
}

#[test]
fn both_route_measurements_must_be_positive_and_finite() {
    for invalid in [-0.1, 0.0, f64::NAN, f64::INFINITY] {
        for python_side in [true, false] {
            let mut routes = complete_routes();
            if python_side { routes[0].python = invalid; } else { routes[0].rust = invalid; }
            assert!(!passes_gate(&complete_contract(0), 0, 0, true, 0.5, 0.6, &routes, &complete_requests()));
        }
    }
}

#[test]
fn decimal_threshold_boundaries_are_exact() {
    for (python, rust) in [(0.300, 0.400), (0.180, 0.270)] {
        let pair = route("R01", python, rust);
        assert!(blocking_regressions(&[pair]).is_empty());
        assert_eq!(fix_item_regressions(&[pair]), ["R01"]);
        assert!(!blocking_regressions(&[route("R01", python, rust + 0.000001)]).is_empty());
    }
}

#[test]
fn unequal_request_totals_cannot_pass_the_error_gate() {
    for (python, rust, py_errors, rs_errors, passes) in [
        (1000, 100, 10, 9, false), (1000, 1000, 10, 9, true),
        (1000, 1000, 10, 11, false), (0, 0, 0, 0, false),
    ] {
        let requests: Vec<_> = complete_routes().iter().map(|row| RouteRequests {
            route_id: row.route_id, python, rust,
        }).collect();
        assert_eq!(passes_gate(&complete_contract(0), rs_errors, py_errors, true, 0.5, 0.6, &complete_routes(), &requests), passes);
    }
}

#[test]
fn absent_contract_evidence_is_not_zero_differences() {
    assert!(!passes_gate(&[], 0, 0, true, 0.5, 0.6, &complete_routes(), &complete_requests()));
}

#[test]
fn incomplete_contract_groups_and_counts_never_pass() {
    let mut evidence = complete_contract(0);
    evidence[0].differences = None;
    assert!(!passes_gate(&evidence, 0, 0, true, 0.5, 0.6, &complete_routes(), &complete_requests()));
    evidence = complete_contract(0);
    evidence.pop();
    assert!(!passes_gate(&evidence, 0, 0, true, 0.5, 0.6, &complete_routes(), &complete_requests()));
}

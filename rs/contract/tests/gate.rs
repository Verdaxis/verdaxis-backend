//! False-pass tests for the decision-68 go/no-go gate.
//!
//! Every test here is written to fail if the gate ever passes on a condition
//! that is missing, unmeasured, or spelled as a placeholder.

use verdaxis_contract::RouteP95;
use verdaxis_contract::{blocking_regressions, fix_item_regressions, passes_gate};

fn route(id: &'static str, python: f64, rust: f64) -> RouteP95 {
    RouteP95 {
        route_id: id,
        python,
        rust,
    }
}

#[test]
fn all_hard_gates_and_speed_pass() {
    let routes = [route("R01", 0.2, 0.1), route("R02", 0.2, 0.3)];
    assert!(passes_gate(0, 3, 5, true, 0.5, 0.6, &routes));
    // Equal 5xx counts are not a rise.
    assert!(passes_gate(0, 5, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn one_unexplained_difference_blocks() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!passes_gate(1, 0, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn a_5xx_rise_blocks_even_with_everything_else_passing() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!passes_gate(0, 6, 5, true, 0.5, 0.6, &routes));
    // One more Rust 503 than Python's, with zero differences, a passing
    // integration suite and a faster mix, is still a no-go.
    assert!(!passes_gate(0, 1, 0, true, 0.1, 10.0, &routes));
}

#[test]
fn a_failing_integration_suite_blocks() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!passes_gate(0, 0, 5, false, 0.5, 0.6, &routes));
}

#[test]
fn a_mix_p95_regression_blocks() {
    let routes = [route("R01", 0.2, 0.1)];
    assert!(!passes_gate(0, 0, 5, true, 0.7, 0.6, &routes));
}

// ------------------------------------------------- per-route speed thresholds

#[test]
fn a_regression_of_exactly_100_ms_does_not_block() {
    // 0.200 s -> 0.300 s is exactly the 100 ms / 50 % boundary: not "more
    // than" either threshold, so the gate still passes and the route is a fix
    // item, not a block.
    assert!(passes_gate(
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
    assert!(!passes_gate(
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
    assert!(passes_gate(
        0,
        0,
        5,
        true,
        0.5,
        0.6,
        &[route("Rcheap", 0.100, 0.150)]
    ));
    assert_eq!(
        fix_item_regressions(&[route("Rcheap", 0.100, 0.150)]),
        ["Rcheap"]
    );
}

#[test]
fn a_route_that_crosses_only_the_relative_threshold_blocks() {
    let routes = [route("Rcheap", 0.100, 0.160)];
    assert_eq!(blocking_regressions(&routes), ["Rcheap"]);
    assert!(!passes_gate(0, 0, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn a_route_that_crosses_only_the_absolute_threshold_blocks() {
    // 0.120 s delta over a slow 1.0 s Python p95: 12 % relative, but over the
    // 100 ms absolute bar.
    let routes = [route("Rslow", 1.000, 1.120)];
    assert_eq!(blocking_regressions(&routes), ["Rslow"]);
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
    assert!(passes_gate(0, 0, 5, true, 0.5, 0.6, &routes));
}

#[test]
fn a_mix_regression_with_every_route_inside_its_limit_still_blocks() {
    // Every route is inside its own threshold, but the weighted mix is worse,
    // so the mix condition blocks.
    let routes = [route("R01", 0.100, 0.150), route("R02", 0.100, 0.150)];
    assert!(blocking_regressions(&routes).is_empty());
    assert!(!passes_gate(0, 0, 5, true, 0.55, 0.50, &routes));
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
    assert!(!passes_gate(
        0,
        0,
        0,
        true,
        0.0,
        0.0,
        &[route("R01", 0.0, 0.0)]
    ));
    assert!(!passes_gate(
        0,
        0,
        0,
        true,
        f64::NAN,
        0.5,
        &[route("R01", 0.2, 0.1)]
    ));
    assert!(!passes_gate(
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
        assert!(!passes_gate(0, 0, 0, true, invalid, 0.6, &routes));
        assert!(!passes_gate(0, 0, 0, true, 0.5, invalid, &routes));
    }
}

#[test]
fn a_zero_python_mix_p95_is_not_a_measurement() {
    // Rust 0.0 against Python 0.0: without a positive Python baseline the mix
    // comparison is vacuous, so the gate must refuse it.
    assert!(!passes_gate(0, 0, 0, true, 0.0, 0.0, &[]));
}

#[test]
fn route_inputs_must_be_finite_before_the_gate() {
    // NaN comparisons are false, so a NaN pair would never "block"; the gate
    // refuses non-finite route input instead of trusting the comparison.
    let nan_route = route("R01", f64::NAN, f64::NAN);
    assert!(!(nan_route.python.is_finite() && nan_route.rust.is_finite()));
    assert!(!passes_gate(0, 0, 0, true, 0.5, 0.6, &[nan_route]));
}

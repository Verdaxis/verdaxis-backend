//! The pure go/no-go gate (founder decision 68, 17 Oct).
//!
//! The gate takes only the **unapproved** difference count: tie-group order the
//! comparator already permits, and AD-01's rate-limit threshold counts, are
//! excluded before this point and are never an allowlist for other failures.
//!
//! Three hard gates must all hold:
//!
//! 1. zero unexplained contract differences;
//! 2. no rise in 5xx/503 responses from Rust against Python on the same
//!    workload;
//! 3. the integration suite passes.
//!
//! Speed blocks only when the overall weighted p95 of Rust is worse than
//! Python's, or any single route's Rust p95 is slower than Python's by more
//! than 100 ms or by more than 50 % — whichever threshold that route crosses
//! first. Smaller per-route regressions do not block; the caller lists them as
//! fix items. Memory is measured and reported (target: Rust at most half of
//! Python for the ported routes) but does not block.

/// A single route's p95 pair, in seconds.
#[derive(Clone, Copy, Debug)]
pub struct RouteP95 {
    pub route_id: &'static str,
    pub python: f64,
    pub rust: f64,
}

/// The per-route regression threshold: a route blocks only when Rust is slower
/// by **more than** 100 ms **or** by **more than** 50 %, whichever it crosses
/// first. A regression exactly at a threshold does not block.
pub const REGRESSION_ABSOLUTE_MS: f64 = 100.0;
pub const REGRESSION_RELATIVE: f64 = 0.5;

/// True when one route's regression is small enough not to block.
///
/// The rule is `rust - python <= 0.1 s` **and** `rust - python <= 0.5 * python`
/// — a route blocks as soon as it crosses either threshold, so a non-block
/// needs to be inside both.
pub fn route_regression_blocks(route: RouteP95) -> bool {
    if route.rust <= route.python {
        return false;
    }
    let delta = route.rust - route.python;
    let absolute = REGRESSION_ABSOLUTE_MS / 1000.0;
    delta > absolute || delta > REGRESSION_RELATIVE * route.python
}

/// The blocking speed regressions among the per-route pairs.
pub fn blocking_regressions(routes: &[RouteP95]) -> Vec<&'static str> {
    routes
        .iter()
        .filter(|route| route_regression_blocks(**route))
        .map(|route| route.route_id)
        .collect()
}

/// The non-blocking regressions, as fix items for the report.
pub fn fix_item_regressions(routes: &[RouteP95]) -> Vec<&'static str> {
    routes
        .iter()
        .filter(|route| route.rust > route.python && !route_regression_blocks(**route))
        .map(|route| route.route_id)
        .collect()
}

/// The decision-68 gate over the measured evidence.
///
/// `unapproved_differences` is the count the comparator still reports after the
/// reviewed tie rules. `rust_5xx` and `python_5xx` are the 5xx/503 counts each
/// stack returned on the same workload. `integration_passed` carries the
/// unchanged integration suite's result. Memory no longer blocks, so it is not
/// an argument; it stays measured and reported by the harness.
#[allow(clippy::too_many_arguments)]
pub fn passes_gate(
    unapproved_differences: u64,
    rust_5xx: u64,
    python_5xx: u64,
    integration_passed: bool,
    rust_mix_p95: f64,
    python_mix_p95: f64,
    routes: &[RouteP95],
) -> bool {
    // A non-finite or non-positive Python mix p95 is not a measurement; the
    // gate refuses it rather than comparing against a placeholder. NaN
    // comparisons are false, so an explicit finiteness check is required.
    let mix_measured = python_mix_p95.is_finite()
        && rust_mix_p95.is_finite()
        && python_mix_p95 > 0.0
        && rust_mix_p95 >= 0.0;
    let routes_measured = routes
        .iter()
        .all(|route| route.python.is_finite() && route.rust.is_finite() && route.python > 0.0);
    let hard_gates =
        unapproved_differences == 0 && rust_5xx <= python_5xx && integration_passed;
    let speed = mix_measured
        && routes_measured
        && rust_mix_p95 <= python_mix_p95
        && blocking_regressions(routes).is_empty();
    hard_gates && speed
}

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
const REGRESSION_ABSOLUTE_US: u128 = 100_000;

/// True when one route's regression is small enough not to block.
///
/// The rule is `rust - python <= 0.1 s` **and** `rust - python <= 0.5 * python`
/// — a route blocks as soon as it crosses either threshold, so a non-block
/// needs to be inside both.
pub fn route_regression_blocks(route: RouteP95) -> bool {
    if route.rust <= route.python {
        return false;
    }
    // Round each measurement once to integer microseconds (half up).
    let python_us = (route.python * 1_000_000.0).round() as u128;
    let rust_us = (route.rust * 1_000_000.0).round() as u128;
    let delta = rust_us.saturating_sub(python_us);
    delta > REGRESSION_ABSOLUTE_US || delta.saturating_mul(2) > python_us
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

/// Expected measured routes and MCP tools, from the committed workload manifest.
pub fn expected_route_ids() -> Vec<String> {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/rust_contract/workload.json"))
            .expect("committed workload manifest must be valid JSON");
    ["routes", "tools"]
        .iter()
        .flat_map(|key| {
            manifest[key]
                .as_array()
                .expect("committed workload route list")
        })
        .map(|row| {
            row["id"]
                .as_str()
                .expect("committed workload route id")
                .to_owned()
        })
        .collect()
}

/// Offered request totals for one route, including transport failures.
#[derive(Clone, Copy, Debug)]
pub struct RouteRequests {
    pub route_id: &'static str,
    pub python: u64,
    pub rust: u64,
}

/// The fixed harness schedule requires equal, nonzero totals per route.
pub fn same_workload(requests: &[RouteRequests]) -> bool {
    let expected = expected_route_ids();
    requests.len() == expected.len()
        && expected
            .iter()
            .all(|id| requests.iter().filter(|row| row.route_id == id).count() == 1)
        && requests
            .iter()
            .all(|row| row.python > 0 && row.python == row.rust)
}

/// A completed simulation group. Missing counts are not zero differences.
#[derive(Clone, Copy, Debug)]
pub struct ContractEvidence {
    pub size: &'static str,
    pub group: &'static str,
    pub cases: u64,
    pub differences: Option<u64>,
}

/// Require one completed result for every committed group at every size.
pub fn contract_differences(evidence: &[ContractEvidence]) -> Option<u64> {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/rust_contract/cases.json")).ok()?;
    let workload: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/rust_contract/workload.json")).ok()?;
    let groups = cases["groups"].as_object()?;
    let sizes = workload["sizes"].as_object()?;
    if evidence.len() != groups.len() * sizes.len() {
        return None;
    }
    let mut total = 0_u64;
    for size in sizes.keys() {
        for group in groups.keys() {
            let mut rows = evidence
                .iter()
                .filter(|row| row.size == size && row.group == group);
            let row = rows.next()?;
            if rows.next().is_some() || row.cases == 0 {
                return None;
            }
            total = total.checked_add(row.differences?)?;
        }
    }
    Some(total)
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
    contract: &[ContractEvidence],
    rust_5xx: u64,
    python_5xx: u64,
    integration_passed: bool,
    rust_mix_p95: f64,
    python_mix_p95: f64,
    routes: &[RouteP95],
    requests: &[RouteRequests],
) -> bool {
    // A non-finite or non-positive Python mix p95 is not a measurement; the
    // gate refuses it rather than comparing against a placeholder. NaN
    // comparisons are false, so an explicit finiteness check is required.
    let mix_measured = python_mix_p95.is_finite()
        && rust_mix_p95.is_finite()
        && python_mix_p95 > 0.0
        && rust_mix_p95 > 0.0;
    let expected = expected_route_ids();
    let routes_measured = routes.len() == expected.len()
        && expected
            .iter()
            .all(|id| routes.iter().filter(|route| route.route_id == id).count() == 1)
        && routes.iter().all(|route| {
            route.python.is_finite()
                && route.rust.is_finite()
                && route.python > 0.0
                && route.rust > 0.0
        });
    let python_requests: u128 = requests.iter().map(|row| u128::from(row.python)).sum();
    let rust_requests: u128 = requests.iter().map(|row| u128::from(row.rust)).sum();
    let errors_measured = same_workload(requests)
        && u128::from(python_5xx) <= python_requests
        && u128::from(rust_5xx) <= rust_requests;
    let error_rate_does_not_rise =
        u128::from(rust_5xx) * python_requests <= u128::from(python_5xx) * rust_requests;
    let hard_gates = contract_differences(contract) == Some(0)
        && errors_measured
        && error_rate_does_not_rise
        && integration_passed;
    let speed = mix_measured
        && routes_measured
        && rust_mix_p95 <= python_mix_p95
        && blocking_regressions(routes).is_empty();
    hard_gates && speed
}

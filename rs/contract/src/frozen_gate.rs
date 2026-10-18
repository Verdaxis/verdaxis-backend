//! Decisions 86/87: validate Rust measurements against pinned Python bytes.
use crate::gate::{RouteP95, WorkloadSize, expected_route_ids, route_regression_blocks};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateOutcome {
    Pass,
    NoGo,
    Inconclusive,
}

#[derive(Debug)]
pub struct PerformanceVerdict {
    pub outcome: GateOutcome,
    pub fix_items: Vec<String>,
    pub blocking_routes: Vec<String>,
    pub mix_regressed: bool,
}

#[derive(Clone, Debug)]
pub struct RustRoute {
    pub route_id: String,
    pub p95: f64,
    pub offered: u64,
    pub successful: u64,
    pub errors_5xx: u64,
}

#[derive(Clone, Debug)]
pub struct RustMeasurement {
    pub size: WorkloadSize,
    pub source_sha: String,
    pub harness_sha: String,
    pub manifest_digest: String,
    pub baseline_sha256: String,
    pub measurement_date: String,
    pub offered_rates: Vec<f64>,
    pub routes: Vec<RustRoute>,
}

#[derive(Deserialize)]
struct GateConfig {
    python_baseline_sha256: String,
    rust_successful_samples_per_route: u64,
    /// Written per-client per-minute limits (tracker decision 78). A route in
    /// this table can never receive more successes in a window than its limit
    /// allows, so decision 96 lowers its floor to 95% of that maximum.
    #[serde(default)]
    rust_route_limits_per_minute: std::collections::BTreeMap<String, u64>,
}
#[derive(Deserialize)]
struct BaselineRoute {
    p95: f64,
    offered: u64,
    successful: u64,
    errors_5xx: u64,
    busy_503: u64,
    error_rate: f64,
}
#[derive(Deserialize)]
struct Baseline {
    routes: BTreeMap<String, BaselineRoute>,
    mix_p95: f64,
    offered: u64,
    errors_5xx: u64,
    source_sha: String,
    harness_sha: String,
    manifest_digest: String,
    identity_attested_by: String,
    measurement_date: String,
    offered_windows: Vec<Window>,
}
#[derive(Deserialize)]
struct Window {
    rps: f64,
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// Per-route successful-sample floors (decisions 87 and 96).
///
/// The measured window is derived from the run's own evidence: total offered
/// requests divided by the offered rate, which is exact for the fixed-rate
/// generator the measurement uses. A missing or non-positive limit, a window
/// that cannot be derived, or a limit naming a route the baseline does not
/// carry is INCONCLUSIVE.
fn route_floors(
    config: &GateConfig,
    baseline: &Baseline,
    offered: u64,
    offered_rates: &[f64],
) -> Option<std::collections::BTreeMap<String, u64>> {
    let full_floor = config.rust_successful_samples_per_route;
    if full_floor < 1000 || config.rust_route_limits_per_minute.is_empty() {
        return None;
    }
    if config
        .rust_route_limits_per_minute
        .values()
        .any(|limit| *limit == 0)
    {
        return None;
    }
    if offered == 0 || offered_rates.is_empty() || !offered_rates.iter().all(finite_positive) {
        return None;
    }
    let window_seconds = offered as f64 / offered_rates[0];
    if !finite_positive(&window_seconds) {
        return None;
    }
    let mut floors = std::collections::BTreeMap::new();
    for id in baseline.routes.keys() {
        let floor = match config.rust_route_limits_per_minute.get(id) {
            Some(limit) => (0.95 * *limit as f64 * window_seconds / 60.0).floor() as u64,
            None => full_floor,
        };
        if floor == 0 {
            return None;
        }
        floors.insert(id.clone(), floor.min(full_floor));
    }
    if config
        .rust_route_limits_per_minute
        .keys()
        .any(|id| !floors.contains_key(id))
    {
        return None;
    }
    Some(floors)
}

fn finite_positive(value: &f64) -> bool {
    value.is_finite() && *value > 0.0
}

/// Missing or invalid evidence always yields Inconclusive, including at large.
pub fn frozen_performance_gate(
    bytes: &[u8],
    config: &str,
    run: &RustMeasurement,
) -> PerformanceVerdict {
    let invalid = || PerformanceVerdict {
        outcome: GateOutcome::Inconclusive,
        fix_items: vec![],
        blocking_routes: vec![],
        mix_regressed: false,
    };
    let Ok(config) = serde_json::from_str::<GateConfig>(config) else {
        return invalid();
    };
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if config.rust_successful_samples_per_route < 1000
        || !hex(&config.python_baseline_sha256, 64)
        || digest != config.python_baseline_sha256
    {
        return invalid();
    }
    let Ok(base) = serde_json::from_slice::<Baseline>(bytes) else {
        return invalid();
    };
    let expected: BTreeSet<_> = expected_route_ids().into_iter().collect();
    let ids: BTreeSet<_> = run.routes.iter().map(|r| r.route_id.clone()).collect();
    if base.routes.keys().cloned().collect::<BTreeSet<_>>() != expected
        || ids != expected
        || run.routes.len() != expected.len()
        || !positive(base.mix_p95)
        || base.offered == 0
        || base.errors_5xx > base.offered
        || !hex(&base.source_sha, 40)
        || !hex(&base.harness_sha, 40)
        || base.identity_attested_by != "PM, tracker decision 87"
        || base.measurement_date != "2026-10-18"
        || !hex(&run.source_sha, 40)
        || !hex(&run.harness_sha, 40)
        || run.manifest_digest != base.manifest_digest
        || run.baseline_sha256 != digest
        || chrono::NaiveDate::parse_from_str(&run.measurement_date, "%Y-%m-%d").is_err()
        || base.routes.values().any(|r| {
            !positive(r.p95)
                || r.successful == 0
                || r.successful > r.offered
                || r.busy_503 > r.errors_5xx
                || r.errors_5xx > r.offered - r.successful
                || !r.error_rate.is_finite()
                || (r.error_rate - r.errors_5xx as f64 / r.offered as f64).abs() > f64::EPSILON
        })
        || run.routes.iter().any(|r| {
            !positive(r.p95) || r.successful > r.offered || r.errors_5xx > r.offered - r.successful
        })
    {
        return invalid();
    }
    let offered: u64 = run.routes.iter().map(|r| r.offered).sum();
    let Some(floors) = route_floors(&config, &base, offered, &run.offered_rates) else {
        return invalid();
    };
    if run
        .routes
        .iter()
        .any(|r| r.successful < floors[&r.route_id])
    {
        return invalid();
    }
    let minimum_rate = base
        .offered_windows
        .iter()
        .map(|w| w.rps)
        .fold(0.0, f64::max);
    if run.offered_rates.is_empty()
        || run
            .offered_rates
            .iter()
            .any(|r| !positive(*r) || *r < minimum_rate)
    {
        return invalid();
    }
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/rust_contract/workload.json"))
            .expect("committed workload");
    let weights: BTreeMap<_, _> = ["routes", "tools"]
        .iter()
        .flat_map(|key| manifest[key].as_array().expect("route array"))
        .map(|row| {
            (
                row["id"].as_str().expect("route id"),
                row["weight_representative"].as_f64().expect("weight"),
            )
        })
        .collect();
    let mut fix_items = vec![];
    let mut blocking_routes = vec![];
    let mut mixed = 0.0;
    let mut offered = 0_u128;
    let mut errors = 0_u128;
    for row in &run.routes {
        let python = base.routes[&row.route_id].p95;
        let blocks = route_regression_blocks(RouteP95 {
            route_id: "",
            python,
            rust: row.p95,
        });
        if blocks && run.size == WorkloadSize::Small {
            blocking_routes.push(row.route_id.clone());
        } else if row.p95 > python {
            fix_items.push(row.route_id.clone());
        }
        mixed += row.p95 * weights[row.route_id.as_str()];
        offered += u128::from(row.offered);
        errors += u128::from(row.errors_5xx);
    }
    let mix_regressed = mixed / weights.values().sum::<f64>() > base.mix_p95;
    let errors_rise = errors * u128::from(base.offered) > u128::from(base.errors_5xx) * offered;
    let blocks = run.size == WorkloadSize::Small
        && (mix_regressed || errors_rise || !blocking_routes.is_empty());
    PerformanceVerdict {
        outcome: if blocks {
            GateOutcome::NoGo
        } else {
            GateOutcome::Pass
        },
        fix_items,
        blocking_routes,
        mix_regressed,
    }
}

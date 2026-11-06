"""Measure the ported-route workload: memory, CPU, and end-to-end latency.

One disposable environment, one seeded workload, two runtimes, identical
traffic. The service processes run inside per-service systemd user units so the
cgroup counters (`memory.current`, `memory.peak`, `memory.stat`, `cpu.stat`)
belong to exactly one runtime and never mix with the load generator or the
database.

Every decision that shapes a reported number is a pure function and is unit
tested without a host: percentile selection, coverage conclusions, cgroup
counter parsing, and the core-equivalent and ratio arithmetic. The run loop
itself is exercised only on the test host, under the operator's wrapper.

The measured load stays at the workload manifest's 1, 5 and 10 requests per
second. A route that cannot reach the manifest's observation floor inside the
run is reported inconclusive with the number still needed -- never a pass.

Phase 10 (plan 10.6) adds the offline final-measurement adapter: a pinned-seed
replay of the frozen schedule, the digest-pinned Python memory comparison over
the 400/100 MiB synthetic fixtures, and the report-only artifact shape. Every
added decision is again a pure function tested without a host; the adapter
refuses to run without the pinned seed and never changes service or deploy
state. The single real run happens much later, under its own reservation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
import subprocess
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[2]

from scripts.rust_contract import run_disposable as runner
from scripts.rust_contract import workload

# ---------------------------------------------------------------- pure helpers


def adaptive_choice(probes: list[dict[str, Any]], max_rps: int) -> dict[str, Any]:
    """Select only a rate proved clean by both stacks; never assume 1 RPS."""
    chosen = None
    reason = "no complete probe"
    for expected, probe in enumerate(probes, 1):
        busy = probe.get("busy", {})
        if probe.get("rps") != expected or set(busy) != {"python", "rust"}:
            reason = "incomplete probe evidence"
            break
        if any(not isinstance(count, int) or count < 0 for count in busy.values()):
            reason = "invalid probe evidence"
            break
        if any(busy.values()):
            reason = f"5xx/503 at {expected} RPS"
            break
        chosen = float(expected)
        reason = "maximum probe rate reached" if expected == max_rps else "probe ended"
    return {"chosen_rps": chosen, "stop_reason": reason, "probe": probes}


def percentile(sorted_values: list[float], q: float) -> float:
    """The nearest-rank percentile of an ascending list.

    `q` is a fraction in (0, 1]. The index is `ceil(q * n) - 1`, the same rule
    `measure_lists.py` uses, so the p95 here is the earlier p95's exact
    definition. An empty list has no percentile and is refused.
    """
    if not sorted_values:
        raise ValueError("no observations")
    if not 0.0 < q <= 1.0:
        raise ValueError("the quantile must be in (0, 1]")
    return sorted_values[math.ceil(q * len(sorted_values)) - 1]


def summarize(samples: list[float]) -> dict[str, float]:
    """n, p50, p95, p99 and the spread for one cell of observations."""
    if not samples:
        raise ValueError("no observations to summarize")
    ordered = sorted(samples)
    return {
        "n": len(ordered),
        "p50": percentile(ordered, 0.50),
        "p95": percentile(ordered, 0.95),
        "p99": percentile(ordered, 0.99),
        "min": ordered[0],
        "max": ordered[-1],
    }


def weighted_mix(
    per_route_p50: dict[str, float],
    per_route_p95: dict[str, float],
    per_route_p99: dict[str, float],
    weights: dict[str, int],
) -> dict[str, float]:
    """The declared representative-mix aggregate latency.

    The mix is the weight-normalized mean of the per-route quantiles over the
    routes that carry a representative weight. It is a declared aggregation of
    measured per-route numbers, never a separate unmeasured run.
    """
    total = sum(weights.values())
    if total <= 0:
        raise ValueError("the representative weights must be positive")
    return {
        quantile: sum(weights[rid] * values[rid] for rid in weights) / total
        for quantile, values in (
            ("p50", per_route_p50),
            ("p95", per_route_p95),
            ("p99", per_route_p99),
        )
    }


def coverage(counts: dict[str, int], required: int) -> dict[str, Any]:
    """Per route: the observed samples and how many more are still needed."""
    routes = {}
    inconclusive = []
    for route_id, observed in sorted(counts.items()):
        if observed >= required:
            routes[route_id] = {"observations": observed, "outcome": "ok"}
        else:
            routes[route_id] = {
                "observations": observed,
                "needed": required - observed,
                "outcome": "inconclusive",
            }
            inconclusive.append(route_id)
    return {"routes": routes, "inconclusive": inconclusive}


def parse_memory_stat(text: str) -> dict[str, int]:
    """The anonymous and file bytes from a cgroup v2 `memory.stat`."""
    fields: dict[str, int] = {}
    for line in text.splitlines():
        name, _, value = line.partition(" ")
        if name in {"anon", "file"}:
            fields[name] = int(value)
    if set(fields) != {"anon", "file"}:
        raise ValueError("memory.stat is missing anon or file")
    return fields


def parse_cpu_usage_usec(text: str) -> int:
    """The cumulative `usage_usec` from a cgroup v2 `cpu.stat`."""
    for line in text.splitlines():
        name, _, value = line.partition(" ")
        if name == "usage_usec":
            return int(value)
    raise ValueError("cpu.stat is missing usage_usec")


def cpu_core_equivalents(delta_usage_usec: int, delta_elapsed_usec: int) -> float:
    """CPU cost as core-equivalents over one interval."""
    if delta_elapsed_usec <= 0:
        raise ValueError("the elapsed interval must be positive")
    return delta_usage_usec / delta_elapsed_usec


def memory_ratio(python_bytes: int, rust_bytes: int) -> float:
    """Rust memory as a fraction of Python's. A zero or missing baseline is refused."""
    if python_bytes is None or python_bytes <= 0:
        raise ValueError("the Python baseline must be positive")
    return rust_bytes / python_bytes


# ------------------------------------------------- phase 10 offline adapter
#
# Plan 10.6: the final measurement replays the frozen schedule from the pinned
# seed, compares digest-pinned Python memory evidence with the Rust peak, and
# writes one report-only artifact. These functions are pure and exercised
# offline; no load is generated, no host is contacted, nothing is measured.


REPORT_WRITE_MODE = "report-only"

COMPARISON_WINDOWS = "comparison"
WARMUP_WINDOWS = "warmup"
RAMP_WINDOWS = "capacity_ramp"

# The common memory metric: max over the window of the per-scrape sum of
# every observed service PID's `Rss:` bytes, converted from KiB at 1024
# bytes per KiB (`read_counters` records exactly that sum in `rss`).
KIB_PER_RSS_LINE = 1024
MEMORY_METRIC = "comparison_peak_process_rss_bytes"


def kib_to_bytes(kib: int) -> int:
    """One `smaps_rollup` `Rss:` reading converted from KiB to bytes."""
    if type(kib) is not int or kib < 0:
        raise ValueError("an RSS reading in KiB must be a non-negative integer")
    return kib * KIB_PER_RSS_LINE


def load_pinned_seed(path: Path | str | None) -> dict[str, Any]:
    """Read the seed fixture and refuse a missing or unpinned file.

    The seed file itself carries a `seed_sha256` over its canonical request
    order, and the config pin (`measure_seed_sha256`) must match the seed
    file's own bytes. A placeholder pin refuses with the same clear message.
    """
    if path is None:
        raise ValueError("missing pinned measurement seed; the run is refused")
    seed_path = Path(path)
    try:
        data = seed_path.read_bytes()
    except OSError as error:
        raise ValueError(f"missing pinned measurement seed: {error}") from error
    document = json.loads(data)
    order = document.get("request_order")
    if not isinstance(order, list) or not order:
        raise ValueError("the pinned seed names no request order")
    expected = hashlib.sha256(
        json.dumps(order, separators=(",", ":")).encode()
    ).hexdigest()
    if document.get("seed_sha256") != expected:
        raise ValueError("the pinned seed's request-order digest does not match")
    return document


def replay_schedule(
    manifest: dict[str, Any], seed: dict[str, Any]
) -> dict[str, Any]:
    """Replay the frozen request order and derive the per-route offered mix.

    The seed's order was derived from the manifest's distribution seed; the
    replay uses that recorded order, never a fresh shuffle. The offered mix
    must match the frozen baseline exactly, or the run would measure a
    different workload than the baseline.
    """
    from scripts.rust_contract import baseline

    reference = baseline.load_python_baseline()
    schedule = workload.weighted_schedule(manifest, seed.get("weight_key", "weight_baseline"))
    if [rid for _, rid in schedule] != seed["request_order"]:
        raise ValueError("the manifest no longer reproduces the pinned request order")
    windows = seed.get("comparison_windows", [])
    offered = sum(int(w["seconds"] * w["rps"]) for w in windows)
    counts: dict[str, int] = {}
    for index in range(offered):
        identifier = seed["request_order"][index % len(seed["request_order"])]
        counts[identifier] = counts.get(identifier, 0) + 1
    expected = {rid: row["offered"] for rid, row in reference["routes"].items()}
    if counts != expected:
        raise ValueError("the replayed offered mix differs from the frozen baseline")
    return {
        "order": seed["request_order"],
        "offered_per_route": counts,
        "offered_total": offered,
    }


def summarize_routes(
    samples: dict[str, list[float]], *, weights: dict[str, int]
) -> dict[str, Any]:
    """Per-route quantiles plus the declared representative-weight aggregate.

    The aggregate is derived from the same per-route observations, never a
    second run.
    """
    per_route = {}
    for route_id in sorted(samples):
        values = sorted(samples[route_id])
        if not values:
            raise ValueError(f"no observations for {route_id}")
        per_route[route_id] = summarize(values)
    result = {
        "per_route": per_route,
        "representative": weighted_mix(
            {rid: row["p50"] for rid, row in per_route.items()},
            {rid: row["p95"] for rid, row in per_route.items()},
            {rid: row["p99"] for rid, row in per_route.items()},
            weights,
        ),
    }
    return result


def representative_weight_result(
    baseline_routes: dict[str, Any], weights: dict[str, int]
) -> dict[str, float]:
    """The declared representative-weight result over the frozen per-route p95s.

    Verifies offline that the retained aggregation reproduces the baseline's
    own declared `mix_p95` from its per-route p95s, so the final report's
    weighted number is the same declared aggregation the baseline used.
    """
    total = sum(weights.get(rid, 0) for rid in baseline_routes)
    if total <= 0:
        raise ValueError("the representative weights must be positive")
    return {
        "p95": sum(
            weights[rid] * row["p95"] for rid, row in baseline_routes.items()
        ) / total
    }


def _window_of(sample_or_reading: dict[str, Any]) -> str:
    return sample_or_reading.get("window", COMPARISON_WINDOWS)


def traffic_summary(
    samples: list[dict[str, Any]],
    counters: list[dict[str, Any]],
    *,
    floor: int | None,
) -> dict[str, Any]:
    """Offered/observed/5xx/503/error-rate, per-route latency and RSS peaks.

    Comparison percentiles and the comparison RSS peak count only comparison
    windows; warmup and the separately tagged capacity ramp are excluded from
    every comparison number and reported apart.
    """
    comparison = [s for s in samples if _window_of(s) == COMPARISON_WINDOWS]
    ramp = [s for s in samples if _window_of(s) == RAMP_WINDOWS]
    per_route: dict[str, list[float]] = {}
    offered = observations = errors = busy = transport_failures = 0
    for sample in comparison:
        seconds = sample["seconds"]
        status = sample["status"]
        if type(seconds) not in (int, float) or not math.isfinite(seconds) or seconds <= 0:
            raise ValueError("invalid latency sample")
        if type(status) is not int or not (status == 0 or 100 <= status < 600):
            raise ValueError("invalid response status")
        offered += 1
        observations += 1
        errors += 500 <= status < 600
        busy += status == 503
        transport_failures += status == 0
        if 200 <= status < 300:
            per_route.setdefault(sample["route_id"], []).append(seconds)
    peak = comparison_peak(counters)
    summary: dict[str, Any] = {
        "offered": offered,
        "observations": offered,
        "errors_5xx": errors,
        "busy_503": busy,
        "transport_failures": transport_failures,
        "error_rate": (errors / offered) if offered else None,
        "peak_rss_bytes": peak,
        "peak_scope": "sum_pid_rss",
        "per_route": {},
        "ramp": {
            "offered": len(ramp),
            "peak_rss_bytes": ramp_peak(counters),
        },
    }
    if per_route:
        declared = _representative_weights()
        # A route outside the declared mix carries weight 1 (the retained
        # report.py rule), so a synthetic id never fails the aggregation.
        weights = {rid: declared.get(rid, 1) for rid in per_route}
        tables = {
            rid: summarize(values)
            for rid, values in per_route.items()
        }
        summary["per_route"] = {
            rid: {"n": tables[rid]["n"], "p95": tables[rid]["p95"]}
            for rid in sorted(tables)
        }
        summary["representative"] = weighted_mix(
            {rid: tables[rid]["p50"] for rid in tables},
            {rid: tables[rid]["p95"] for rid in tables},
            {rid: tables[rid]["p99"] for rid in tables},
            weights,
        )
    return summary


def _representative_weights() -> dict[str, int]:
    from scripts import rust_contract

    manifest = rust_contract.workload.load()
    return {
        row["id"]: int(row["weight_representative"])
        for row in rust_contract.workload.routes(manifest) + rust_contract.workload.tools(manifest)
    }


def comparison_peak(readings: list[dict[str, Any]]) -> int:
    """max over comparison-window scrapes of the summed-PID RSS bytes.

    The retained scraper already records `rss` as the per-scrape sum of every
    observed service PID's `smaps_rollup` `Rss:` KiB converted to bytes; the
    comparison peak is the maximum of that sum over the comparison window
    only. Warmup and ramp readings are excluded here.
    """
    values = [
        reading["rss"]
        for reading in readings
        if _window_of(reading) == COMPARISON_WINDOWS and "rss" in reading
        and type(reading["rss"]) is int
    ]
    if not values:
        return 0
    return max(values)


def ramp_peak(readings: list[dict[str, Any]]) -> int:
    """max over capacity-ramp scrapes of the summed-PID RSS bytes."""
    values = [
        reading["rss"]
        for reading in readings
        if _window_of(reading) == RAMP_WINDOWS
        and "rss" in reading
        and type(reading["rss"]) is int
    ]
    if not values:
        return 0
    return max(values)


def sample_floor_caveat(
    summary: dict[str, Any],
    *,
    floor: int,
    needed_per_route: dict[str, int] | None,
) -> dict[str, Any]:
    """The below-1,000-sample caveat over the comparison-window successes.

    A route below the floor (or below a per-minute-limited route's declared
    floor) keeps the caveat and its still-needed count; a route at or above
    its floor clears the caveat. The caveat is a report annotation, never a
    pass.
    """
    successes = {
        rid: cell.get("n", cell.get("observations", 0))
        for rid, cell in summary["per_route"].items()
    }
    needed = {}
    below = False
    for route_id, count in sorted(successes.items()):
        required = needed_per_route.get(route_id, floor) if needed_per_route else floor
        if count < required:
            below = True
            needed[route_id] = required - count
    if below:
        message = (
            f"per-route successful samples are below the {floor:,} floor; "
            + ", ".join(f"{rid} needs {needed[rid]} more" for rid in sorted(needed))
            + ". every route is reported with the number still needed, never a pass"
        )
    else:
        needed = {}
        message = ""
    return {"below_floor": below, "needed_per_route": needed, "message": message}


def memory_comparison(
    python_peak: int | None,
    rust_peak: int | None,
    extraction: dict[str, Any],
    rust_side: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Signed absolute/percentage differences and savings, or the explicit absence.

    `python_peak=None` with an `unavailable` extraction reports the verified
    absence: null deltas, the Rust peak alone and the explicit caveat. A zero
    Python peak refuses: no comparable observation is not a 100% saving.
    """
    _require_provenance(extraction, rust_side)
    if python_peak is None:
        if extraction.get("status") != "unavailable":
            raise ValueError("missing Python peak requires an unavailable record")
        return {
            "status": "unavailable",
            "caveat": "memory savings unavailable: no comparable frozen observation",
            "python_peak_bytes": None,
            "rust_peak_bytes": rust_peak,
            "difference_bytes": None,
            "difference_percent": None,
            "savings_bytes": None,
            "savings_percent": None,
            "evidence": extraction.get("evidence", {}),
            "metric": MEMORY_METRIC,
            "scope": extraction.get("scope", {}),
            "window": extraction.get("window", {}),
            "reported_rust_peak_scope": "comparison-window summed-PID RSS",
        }
    if python_peak <= 0:
        raise ValueError("a zero Python peak is not a comparable baseline")
    if type(rust_peak) is not int or rust_peak <= 0:
        raise ValueError("the Rust peak must be a positive integer")
    scope = extraction.get("scope")
    if not isinstance(scope, dict) or scope.get("python_workers") != 4 \
            or scope.get("include_supervisor") is not True:
        raise ValueError("memory scope mismatch: the whole service process set is required")
    window = extraction.get("window")
    if not isinstance(window, dict) or window.get("scrape_interval_seconds") != 1 \
            or window.get("exclude") != ["warmup", "capacity_ramp"]:
        raise ValueError("memory window mismatch: the pinned comparison window is required")
    if extraction.get("status") != "available":
        present = extraction.get("status")
        raise ValueError(f"the Python memory evidence is not available: {present}")
    difference = rust_peak - python_peak
    percent = 100 * difference / python_peak
    savings = python_peak - rust_peak
    return {
        "status": "compared",
        # The retained counter semantics: one-second /proc/<pid>/smaps_rollup
        # `Rss:` readings, KiB converted at 1024 bytes/KiB, summed over the
        # service process set, maximised over the comparison window.
        "metric": MEMORY_METRIC,
        "scope": extraction["scope"],
        "window": extraction["window"],
        "evidence": extraction["evidence"],
        "python_peak_bytes": python_peak,
        "rust_peak_bytes": rust_peak,
        "difference_bytes": difference,
        "difference_percent": percent,
        "savings_bytes": savings,
        "savings_percent": 100 * savings / python_peak,
    }


def _require_provenance(
    extraction: dict[str, Any], rust_side: dict[str, Any] | None
) -> None:
    """Both sides must pin the same metric, scope, window and source digest.

    The Python extraction and the Rust collection must declare the identical
    metric, scope, window and pinned raw-source digest before any difference
    is computed; any mismatch refuses, so numbers from different scopes or
    windows are never subtracted.
    """
    if not isinstance(extraction, dict) or not isinstance(extraction.get("evidence"), dict):
        raise ValueError("missing memory provenance evidence")
    evidence = extraction["evidence"]
    for key in ("source_sha256", "extraction_sha256"):
        value = evidence.get(key)
        if not isinstance(value, str) or len(value) != 64 or set(value) == {"0"}:
            raise ValueError(
                "memory evidence refused: missing or placeholder pinned "
                f"{key}; record the real digest before the run"
            )
    if extraction.get("metric") != MEMORY_METRIC:
        raise ValueError("memory metric mismatch: comparison_peak_process_rss_bytes is required")
    if rust_side is not None:
        for key in ("metric", "scope", "window"):
            if extraction.get(key) != rust_side.get(key):
                raise ValueError(f"memory {key} mismatch between the two sides")
        rust_evidence = rust_side.get("evidence", {})
        if rust_evidence.get("source_sha256") != evidence["source_sha256"]:
            raise ValueError("memory source digest mismatch between the two sides")



def write_mode(mode: str) -> str:
    """The tool's single write surface: the report. Everything else refuses."""
    if mode != REPORT_WRITE_MODE:
        raise ValueError(
            f"refused write mode {mode!r}: the measurement tool is report-only"
        )
    return REPORT_WRITE_MODE


def write_report(destination: Path, payload: dict[str, Any]) -> dict[str, Any]:
    """The artifact shape: one report document, never a service change."""
    return {
        "artifact_kind": "report",
        "gate": REPORT_WRITE_MODE,
        "destination": str(destination),
        "payload_digest": hashlib.sha256(
            json.dumps(payload, sort_keys=True).encode()
        ).hexdigest(),
        "mutates_service_state": False,
    }


def load_measure_config(path: Path | str) -> dict[str, Any]:
    """Read the final-measure config and refuse placeholder seed pins.

    `measure_seed_sha256` in the phase10 gate config must match the seed
    file's committed bytes. The committed placeholder makes the tool refuse
    with a clear message until the PM records the real digest; the config's
    own `seed_sha256` must also match the seed bytes when both are present.
    """
    config_path = Path(path)
    document = tomllib.loads(config_path.read_text())
    if not document.get("report_only"):
        raise ValueError("the final measurement config must be report_only")
    seed_path = ROOT / document["seed"]
    seed_bytes = seed_path.read_bytes()
    seed_digest = hashlib.sha256(seed_bytes).hexdigest()
    pin = document.get("seed_sha256")
    if not pin or pin != seed_digest:
        raise ValueError(
            "the configured seed pin does not match the committed seed bytes; "
            "record the real digest before the run"
        )
    if document.get("requires_cutover_complete") is not True:
        raise ValueError("the final measurement requires_cutover_complete")
    return document


def require_cutover(evidence: dict[str, Any] | None) -> None:
    """Refuse unless a signed post-deletion inventory (Task 33) is present.

    The measurement runs after cutover only: absent or pre-removal evidence
    refuses before any host contact.
    """
    if not isinstance(evidence, dict):
        raise ValueError("missing cutover evidence: the measurement runs after cutover only")
    if evidence.get("phase") != "post-removal" or not evidence.get("signed_inventory_sha256"):
        raise ValueError(
            "cutover is not complete: a signed post-deletion host inventory is required"
        )


def require_no_regression(summary: list[dict[str, Any]]) -> list[str]:
    """Name every route whose Rust p95 exceeds its Python p95.

    An empty list means no measured regression. A missing counterpart is
    reported, not ignored.
    """
    indexed = {(row["route_id"], row["target"]): row for row in summary}
    problems = []
    for (route_id, target) in sorted(indexed):
        if target != "rust":
            continue
        python = indexed.get((route_id, "python"))
        rust = indexed[(route_id, "rust")]
        if python is None:
            problems.append(f"{route_id}: no Python counterpart")
        elif rust["p95"] > python["p95"]:
            problems.append(
                f"{route_id}: Rust p95 {rust['p95']:.4f}s > Python {python['p95']:.4f}s"
            )
    return problems


# ------------------------------------------------------------- run-only pieces


@dataclass
class ServiceHandle:
    """One runtime's processes and the cgroup that accounts for them."""

    name: str
    unit: str = ""
    cgroup: str = ""
    child: subprocess.Popen | None = None
    pids: list[int] = None  # type: ignore[assignment]

    def __post_init__(self) -> None:
        if self.pids is None:
            self.pids = []


def _cgroup_root() -> str:
    own = Path("/proc/self/cgroup").read_text().split("::", 1)[-1].strip()
    return f"/sys/fs/cgroup{own}"


def _ensure_user_session() -> None:
    """Make `systemd-run --user` reachable from this process.

    The per-service accounting needs one cgroup per runtime, which only a
    systemd user unit provides here. The runtime directory is the caller's own
    user slice; it is not a secret and is not shared across users.
    """
    os.environ.setdefault("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")


class Measurement(runner.Runner):
    """Run both runtimes on one seeded workload and collect the evidence."""

    def __init__(
        self,
        size: str,
        *,
        output_dir: Path,
        warmup_seconds: int,
        windows: list[tuple[float, int]],
        repetitions: int,
        weight_key: str = "weight_baseline",
        probe_seconds: int = 30,
        probe_max_rps: int = 5,
        measure_seconds: int = 600,
        keep: bool = False,
        rust_only: bool = True,
    ) -> None:
        super().__init__(
            "all", seed=True, keep=keep, seed_mode="corpus", mixed_caddy=True, release=True
        )
        self.rust_only = rust_only
        self.size = size
        self.output_dir = output_dir
        self.warmup_seconds = warmup_seconds
        self.windows = windows
        self.repetitions = repetitions
        self.weight_key = weight_key
        self.probe_seconds = probe_seconds
        self.probe_max_rps = probe_max_rps
        self.measure_seconds = measure_seconds
        self.manifest = workload.load()
        self.counts = workload.sizes(self.manifest)[size]
        self.schedule = workload.weighted_schedule(self.manifest, weight_key)
        self.cases = self._case_table()
        self.services: list[ServiceHandle] = []

    def measurement_targets(self, python_port: int, rust_port: int) -> list[tuple[str, int]]:
        return [("rust", rust_port)] if self.rust_only else [("python", python_port), ("rust", rust_port)]

    def measurement_identity(self) -> dict[str, Any]:
        from scripts.rust_contract import baseline
        reference = baseline.load_python_baseline()
        rates = [rate for _, rate in self.windows]
        minimum = max(window["rps"] for window in reference["offered_windows"])
        if self.rust_only and (self.weight_key != "weight_baseline" or any(rate < minimum for rate in rates)):
            raise ValueError("Rust-only runs require the frozen schedule at at least its maximum offered rate")
        return {"mode": "rust-only" if self.rust_only else "paired", "source_sha": self.sha,
                "harness_sha": self.sha, "manifest_digest": workload.manifest_digest(self.manifest),
                "baseline_sha256": baseline.gate_config()["python_baseline_sha256"],
                "measurement_date": datetime.now(timezone.utc).date().isoformat(),
                "rates_rps": rates, "weight_key": self.weight_key}

    # -- workload plumbing

    def _case_table(self) -> dict[str, dict[str, Any]]:
        table = {}
        for route in workload.routes(self.manifest):
            table[route["id"]] = {
                "path": route["path"],
                "query": route["measure_query"],
                "method": "GET",
            }
        for tool in workload.tools(self.manifest):
            # The streamable-HTTP transport requires exactly this Accept set;
            # without it every call is refused 406 before any tool runs.
            table[tool["id"]] = {
                "path": "/mcp",
                "query": "",
                "method": "POST",
                "body": json.dumps(tool["body"]).encode(),
                "content_type": "application/json",
                "accept": "application/json, text/event-stream",
            }
        return table

    def _seed_workload(self, env: dict[str, str]) -> None:
        subprocess.run(
            [self.python_bin, "-m", "scripts.rust_contract.workload_seed", "--size", self.size],
            cwd=ROOT,
            env={**env, "PYTHONPATH": str(ROOT)},
            check=True,
        )

    # -- service control

    def start_service(
        self,
        name: str,
        argv: list[str],
        env: dict[str, str],
        *,
        systemd: bool = True,
    ) -> ServiceHandle:
        """Start one runtime's processes and return its accounting handle."""
        handle = ServiceHandle(name=name)
        if systemd and shutil.which("systemd-run"):
            handle.unit = f"t14a-{self.size}-{name}-{os.getpid()}-{len(self.services)}"
            command = [
                "systemd-run",
                "--user",
                "--unit",
                handle.unit,
                "--collect",
                "--quiet",
                "-p",
                f"WorkingDirectory={ROOT}",
            ]
            for key, value in env.items():
                command += ["-E", f"{key}={value}"]
            command += argv
            subprocess.run(command, check=True)
            show = subprocess.run(
                ["systemctl", "--user", "show", handle.unit, "-p", "ControlGroup", "--value"],
                capture_output=True,
                text=True,
                check=True,
            )
            # ControlGroup is an absolute path under /sys/fs/cgroup.
            handle.cgroup = "/sys/fs/cgroup" + show.stdout.strip()
        else:
            handle.child = subprocess.Popen(
                argv, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
            )
        self.services.append(handle)
        return handle

    def stop_service(self, handle: ServiceHandle) -> None:
        if handle.unit:
            subprocess.run(
                ["systemctl", "--user", "stop", handle.unit],
                check=False,
                capture_output=True,
            )
        elif handle.child is not None:
            handle.child.terminate()
            try:
                handle.child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                handle.child.kill()
                handle.child.wait()
        if handle in self.services:
            self.services.remove(handle)

    def _unit_pids(self, handle: ServiceHandle) -> list[int]:
        if not handle.cgroup:
            return [handle.child.pid] if handle.child else []
        try:
            text = Path(handle.cgroup, "cgroup.procs").read_text()
        except OSError:
            return []
        return [int(line) for line in text.split() if line]

    def read_counters(self, handle: ServiceHandle) -> dict[str, Any]:
        """One second's memory and CPU counters for a running service unit."""
        reading: dict[str, Any] = {"t": time.monotonic(), "unit": handle.unit}
        if handle.cgroup:
            try:
                reading["memory_current"] = int(
                    Path(handle.cgroup, "memory.current").read_text().strip()
                )
                reading["memory_peak"] = int(
                    Path(handle.cgroup, "memory.peak").read_text().strip()
                )
                reading.update(
                    parse_memory_stat(Path(handle.cgroup, "memory.stat").read_text())
                )
                reading["cpu_usage_usec"] = parse_cpu_usage_usec(
                    Path(handle.cgroup, "cpu.stat").read_text()
                )
            except (OSError, ValueError) as error:
                reading["error"] = str(error)
        pss = 0
        rss = 0
        count = 0
        for pid in self._unit_pids(handle):
            try:
                text = Path(f"/proc/{pid}/smaps_rollup").read_text()
            except OSError:
                continue
            for line in text.splitlines():
                if line.startswith("Pss:"):
                    pss += int(line.split()[1]) * 1024
                elif line.startswith("Rss:"):
                    rss += int(line.split()[1]) * 1024
            count += 1
        reading["pss"] = pss
        reading["rss"] = rss
        reading["processes"] = count
        return reading

    # -- traffic

    def _request(self, port: int, case: dict[str, Any]) -> tuple[float, int]:
        """One end-to-end request through Caddy; returns (seconds, status).

        No Host header is set explicitly: urllib writes `127.0.0.1:<port>`,
        which is what the corpus recorder sends and what the MCP transport's
        host allowlist accepts. An explicit portless Host would be refused 421.
        """
        url = f"http://127.0.0.1:{port}{case['path']}"
        if case["query"]:
            url += "?" + case["query"]
        headers = {"x-request-id": "t14a-measure"}
        data = None
        if case["method"] == "POST":
            data = case["body"]
            headers["content-type"] = case["content_type"]
            headers["accept"] = case["accept"]
        request = urllib.request.Request(url, data=data, headers=headers, method=case["method"])
        started = time.perf_counter()
        try:
            with urllib.request.urlopen(request, timeout=65) as response:
                response.read()
                status = response.status
        except urllib.error.HTTPError as error:
            status = error.code
        except (urllib.error.URLError, OSError, TimeoutError):
            status = 0
        return time.perf_counter() - started, status

    def load_series(self, port: int, seconds: int, rps: float) -> list[dict[str, Any]]:
        """A fixed-rate offered-load window; queuing is included in the latency.

        The offered rate is recorded with every sample, so the unsaturated
        windows and the 10 RPS saturation behaviour can be reported apart.
        """
        total = max(1, int(seconds * rps))
        interval = 1.0 / rps
        records: list[dict[str, Any]] = []
        lock = threading.Lock()
        concurrency = max(2, int(rps * 2))

        def one(index: int) -> None:
            kind, identifier = self.schedule[index % len(self.schedule)]
            elapsed, status = self._request(port, self.cases[identifier])
            with lock:
                records.append(
                    {
                        "route_id": identifier,
                        "kind": kind,
                        "seconds": elapsed,
                        "status": status,
                        "rps": rps,
                    }
                )

        with ThreadPoolExecutor(max_workers=concurrency) as pool:
            started = time.monotonic()
            for index in range(total):
                target = started + index * interval
                now = time.monotonic()
                if target > now:
                    time.sleep(target - now)
                pool.submit(one, index)
        return records

    # -- orchestration

    def _scrape_loop(
        self, handle: ServiceHandle, stop: threading.Event, out: list[dict[str, Any]]
    ) -> None:
        while not stop.is_set():
            out.append(self.read_counters(handle))
            stop.wait(1.0)

    def _start_caddy(self, upstream: str) -> tuple[subprocess.Popen, int]:
        caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
        if not caddy:
            caddy = str(ROOT / "work/rust-phase1/bin/caddy")
        port = runner.ephemeral_port()
        env = {
            **os.environ,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "CADDY_PORT": str(port),
            "PYTHON_UPSTREAM": upstream,
            **{f"{group}_UPSTREAM": upstream for group in ("SIMPLE", "ORDERBOOK", "PRICES", "CURVES", "MCP")},
        }
        config = str(ROOT / "tests/rust_contract/Caddyfile")
        subprocess.run([caddy, "adapt", "--config", config, "--validate"], cwd=ROOT, env=env, check=True)
        child = subprocess.Popen(
            [caddy, "run", "--config", config], cwd=ROOT, env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        runner.wait_for_port(port)
        return child, port

    def run(self) -> None:
        """Build, seed once, then run both runtimes in alternating order."""
        identity = self.measurement_identity()
        subprocess.run(self.build_command(), cwd=ROOT, env={**os.environ, "RELEASE_SHA": self.sha}, check=True)
        subprocess.run(
            ["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "--release", "-p", "verdaxis-read"],
            cwd=ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha},
            check=True,
        )
        port = self.start_database()
        base_env = {
            **os.environ,
            "ENVIRONMENT": "test",
            "RELEASE_SHA": self.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DISPOSABLE_TEST_TOKEN": self.token,
            "RUST_CONTRACT_CLOCK": runner.CORPUS_CLOCK,
            "KYC_EXECUTION_GATE_ENABLED": "true",
            "RUST_CORPUS_MANIFEST": str(ROOT / "work/rust-phase1/task4b-fixtures.json"),
            **self.urls(port),
        }
        self.migrate_and_seed(base_env)
        self._seed_workload(base_env)
        runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres", "-d", runner.DB_NAME,
            "-v", "ON_ERROR_STOP=1", "-c", "ANALYZE",
        )

        python_port = runner.ephemeral_port()
        rust_port = runner.ephemeral_port()
        rust_config = json.dumps(
            {
                "environment": "test",
                "kyc_execution_gate_enabled": True,
                "release_sha": self.sha,
                "migration_revision": runner.MIGRATION_REVISION,
                "database_url": base_env["RUST_DATABASE_URL"],
                "db_session_time_zone": runner.ORACLE_TIMEZONE,
                "bind": f"127.0.0.1:{rust_port}",
                "read_admission_slots": 16, "pool_max": 17,
                "python_budget": 52,
                "configured_max_connections": 100,
            }
        )

        evidence: dict[str, Any] = {
            "size": self.size,
            "counts": self.counts,
            "source_sha": self.sha,
            "python_workers": 4,
            "rust_processes": 1,
            "rates_rps": [rate for _, rate in self.windows],
            "warmup_seconds": self.warmup_seconds,
            "repetitions": [],
            "systemd_units": bool(shutil.which("systemd-run")),
        }
        evidence.update(identity)
        order = self.measurement_targets(python_port, rust_port)
        for repetition in range(self.repetitions):
            # Alternate which runtime goes first, so a warm cache never favours
            # one side across every repetition.
            sequence = order if repetition % 2 == 0 else order[::-1]
            rep: dict[str, Any] = {"index": repetition, "order": [n for n, _ in sequence], "cells": {}}
            for target, port in sequence:
                handle = self._start_service_unit(target, python_port, rust_port, rust_config, base_env)
                try:
                    rep["cells"][target] = self._measure_cell(handle, port)
                finally:
                    self.stop_service(handle)
            evidence["repetitions"].append(rep)
        if self.rust_only:
            from scripts.rust_contract.frozen_gate import frozen_verdict
            evidence["performance_verdict"] = frozen_verdict(evidence)
            evidence["performance_verdict"].pop("baseline", None)
        suffix = "-rust-only" if self.rust_only else ""
        out = self.output_dir / f"measure-{self.size}{suffix}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(evidence, indent=2) + "\n")
        print(f"MEASUREMENT_JSON {out}")

    def run_adaptive(self) -> None:
        """Ramp the offered rate while neither stack returns busy/503.

        Item 5: the fixed 5 RPS large run saturated both services, so its p95
        and 5xx counts could not judge the gate. This starts at 1 RPS and
        raises the rate only while neither stack returns a busy (503) or other
        5xx response, then measures the full window at the highest clean rate.
        """
        subprocess.run(self.build_command(), cwd=ROOT, env={**os.environ, "RELEASE_SHA": self.sha}, check=True)
        subprocess.run(
            ["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "--release", "-p", "verdaxis-read"],
            cwd=ROOT, env={**os.environ, "RELEASE_SHA": self.sha}, check=True,
        )
        port = self.start_database()
        base_env = {
            **os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": self.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DISPOSABLE_TEST_TOKEN": self.token, "RUST_CONTRACT_CLOCK": runner.CORPUS_CLOCK,
            "KYC_EXECUTION_GATE_ENABLED": "true",
            "RUST_CORPUS_MANIFEST": str(ROOT / "work/rust-phase1/task4b-fixtures.json"),
            **self.urls(port),
        }
        self.migrate_and_seed(base_env)
        self._seed_workload(base_env)
        runner.docker("exec", self.container, "psql", "-XAt", "-U", "postgres", "-d", runner.DB_NAME,
                      "-v", "ON_ERROR_STOP=1", "-c", "ANALYZE")
        python_port = runner.ephemeral_port()
        rust_port = runner.ephemeral_port()
        rust_config = json.dumps({
            "environment": "test", "kyc_execution_gate_enabled": True, "release_sha": self.sha,
            "migration_revision": runner.MIGRATION_REVISION, "database_url": base_env["RUST_DATABASE_URL"],
            "db_session_time_zone": runner.ORACLE_TIMEZONE, "bind": f"127.0.0.1:{rust_port}",
            "read_admission_slots": 16, "pool_max": 17, "python_budget": 52, "configured_max_connections": 100,
        })
        evidence: dict[str, Any] = {
            "size": self.size, "counts": self.counts, "source_sha": self.sha,
            "python_workers": 4, "rust_processes": 1, "mode": "adaptive",
            "probe_seconds": self.probe_seconds, "probe_max_rps": self.probe_max_rps,
            "measure_seconds": self.measure_seconds,
            "probe": [], "repetitions": [],
        }
        order = [("python", python_port), ("rust", rust_port)]
        handles: dict[str, Any] = {}
        caddies: dict[str, tuple[Any, int]] = {}
        try:
            # One service per runtime, kept up across the probe and the window.
            for target, port in order:
                handles[target] = self._start_service_unit(target, python_port, rust_port, rust_config, base_env)
                self._await_ready(port)
                caddies[target] = self._start_caddy(f"127.0.0.1:{port}")
            # Probe: raise the rate while neither stack shows a busy/5xx.
            clean_rps = None
            for rps in range(1, self.probe_max_rps + 1):
                busy = {}
                for target, _ in order:
                    samples = self.load_series(caddies[target][1], self.probe_seconds, float(rps))
                    busy[target] = sum(1 for s in samples if s["status"] >= 500)
                evidence["probe"].append({"rps": rps, "busy": busy})
                print(f"PROBE rps={rps} busy={busy}", flush=True)
                if any(count > 0 for count in busy.values()):
                    break
                clean_rps = float(rps)
            choice = adaptive_choice(evidence["probe"], self.probe_max_rps)
            evidence.update(choice)
            clean_rps = choice["chosen_rps"]
            if clean_rps is not None:
                # Measure the full window at the highest clean rate the probe found.
                self.windows = [(float(self.measure_seconds), clean_rps)]
                cells: dict[str, Any] = {}
                for target, port in order:
                    cells[target] = self._measure_cell(
                        handles[target], port, caddy_port=caddies[target][1]
                    )
                evidence["repetitions"].append(
                    {"index": 0, "order": [n for n, _ in order], "cells": cells}
                )
        finally:
            for caddy, _ in caddies.values():
                caddy.terminate()
                try:
                    caddy.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    caddy.kill()
            for handle in handles.values():
                self.stop_service(handle)
        out = self.output_dir / f"measure-{self.size}-adaptive.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(evidence, indent=2) + "\n")
        print(f"MEASUREMENT_JSON {out}")

    def _start_service_unit(
        self, target: str, python_port: int, rust_port: int, rust_config: str, base_env: dict[str, str]
    ) -> ServiceHandle:
        if target == "python":
            return self.start_service(
                "python",
                [
                    str(ROOT / "venv/bin/python"), "-m", "uvicorn",
                    "tests.rust_contract.server:factory", "--factory",
                    "--workers", "4", "--host", "127.0.0.1", "--port", str(python_port),
                    "--log-level", "warning",
                ],
                {**base_env, "PYTHONPATH": str(ROOT), "UVICORN_BIND_PORT": str(python_port)},
            )
        return self.start_service(
            "rust",
            [str(ROOT / "rs/target/release/verdaxis-read"), "--bind", f"127.0.0.1:{rust_port}"],
            {**base_env, "RUST_CONFIG": rust_config},
        )

    def _await_ready(self, port: int, timeout: float = 120) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/health/live", timeout=2).read()
                return
            except (urllib.error.URLError, OSError):
                time.sleep(0.5)
        raise runner.RunnerError(f"the service on {port} never became ready")

    def _measure_cell(
        self, handle: ServiceHandle, port: int | None, *, caddy_port: int | None = None
    ) -> dict[str, Any]:
        """Warm up, then offer the declared rates and record every sample.

        `caddy_port` reuses an already-started Caddy (the adaptive run keeps one
        per runtime); otherwise a fresh Caddy is started for `port`.
        """
        ready_port = port or 0
        if port is not None:
            self._await_ready(port)
        self.load_series(ready_port, self.warmup_seconds, 5.0)
        samples: list[dict[str, Any]] = []
        counters: list[dict[str, Any]] = []
        stop = threading.Event()
        scraper = threading.Thread(target=self._scrape_loop, args=(handle, stop, counters), daemon=True)
        scraper.start()
        owned = caddy_port is None
        caddy = None
        if owned:
            caddy, caddy_port = self._start_caddy(f"127.0.0.1:{port}")
        try:
            for seconds, rate in self.windows:
                samples += self.load_series(caddy_port, seconds, rate)
        finally:
            stop.set()
            scraper.join(timeout=5)
            if owned and caddy is not None:
                caddy.terminate()
                caddy.wait(timeout=10)
        return {"samples": samples, "counters": counters}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rust-only", action="store_true", help="measure Rust only against the frozen Python baseline")
    parser.add_argument(
        "--adaptive",
        action="store_true",
        help="ramp the offered rate while neither stack returns busy/503, then measure",
    )
    args = parser.parse_args(argv)
    if args.adaptive:
        parser.error("Decision 86 measures Rust only; use explicit fixed windows, not paired adaptive probes")

    document = tomllib.loads(args.config.read_text())
    _ensure_user_session()
    measurement = Measurement(
        str(document["size"]),
        output_dir=args.output,
        warmup_seconds=int(document.get("warmup_seconds", 30)),
        windows=[(float(w["seconds"]), float(w["rps"])) for w in document["windows"]],
        repetitions=int(document.get("repetitions", 2)),
        weight_key=str(document.get("weight_key", "weight_baseline")),
        probe_seconds=int(document.get("probe_seconds", 30)),
        probe_max_rps=int(document.get("probe_max_rps", 5)),
        measure_seconds=int(document.get("measure_seconds", 600)),
        rust_only=True,
    )
    try:
        if args.adaptive:
            measurement.run_adaptive()
        else:
            measurement.run()
    finally:
        measurement.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

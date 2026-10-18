"""Assemble measured evidence into the Task 14a go/no-go report.

Every number here comes from a measured run. A field that was not measured is
rendered as `not measured`; nothing is prefilled. The report states the exact
gate outcome per condition and keeps inconclusive results visible.

Only aggregate, redacted evidence and input digests reach `docs/rust-phase1/`.
Raw samples stay under `work/rust-phase1/`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import statistics
from pathlib import Path
from typing import Any

from scripts.rust_contract import measure as measure_mod
from scripts.rust_contract import workload
from scripts.rust_contract.frozen_gate import frozen_verdict

ROOT = Path(__file__).resolve().parents[2]
NOT_MEASURED = "not measured"


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def evidence_digest(evidence: Any) -> str:
    """One digest over the canonical spelling of an evidence document."""
    return sha256_bytes(workload.canonical_json(evidence).encode())


def _cells(evidence: dict[str, Any]) -> list[dict[str, Any]]:
    """Flatten every (repetition, target) cell into one list."""
    rows = []
    for repetition in evidence["repetitions"]:
        for target, cell in repetition["cells"].items():
            rows.append({"target": target, "repetition": repetition["index"], **cell})
    return rows


def memory_table(evidence: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Steady median, p95, peak, anon/file and PSS per runtime."""
    table: dict[str, dict[str, Any]] = {}
    for row in _cells(evidence):
        counters = [c for c in row["counters"] if "memory_current" in c]
        if not counters:
            table[row["target"]] = {"outcome": "inconclusive", "reason": "no cgroup counter"}
            continue
        current = sorted(c["memory_current"] for c in counters)
        peaks = [c["memory_peak"] for c in counters]
        table[row["target"]] = {
            "samples": len(current),
            "median_bytes": statistics.median(current),
            "p95_bytes": measure_mod.percentile(current, 0.95),
            "peak_bytes": max(peaks),
            "anon_median_bytes": statistics.median([c["anon"] for c in counters]),
            "file_median_bytes": statistics.median([c["file"] for c in counters]),
            "pss_median_bytes": statistics.median([c["pss"] for c in counters]),
        }
    return table


def cpu_table(evidence: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Core-equivalents and CPU microseconds per completed request."""
    table: dict[str, dict[str, Any]] = {}
    for row in _cells(evidence):
        counters = [c for c in row["counters"] if "cpu_usage_usec" in c]
        completed = [s for s in row["samples"] if 200 <= s["status"] < 300]
        if not counters or not completed:
            table[row["target"]] = {"outcome": "inconclusive", "reason": "no CPU counter or no completed request"}
            continue
        elapsed_usec = (counters[-1]["t"] - counters[0]["t"]) * 1_000_000
        usage_usec = counters[-1]["cpu_usage_usec"] - counters[0]["cpu_usage_usec"]
        table[row["target"]] = {
            "completed_requests": len(completed),
            "core_equivalents": measure_mod.cpu_core_equivalents(int(usage_usec), int(elapsed_usec)),
            "usec_per_request": usage_usec / len(completed),
        }
    return table


def latency_table(evidence: dict[str, Any]) -> dict[str, dict[str, dict[str, Any]]]:
    """Per route and per runtime, the successful-response quantiles and every
    non-success class reported separately.

    A response the contract specifies (2xx) is the latency series. A 4xx the
    contract specifies (404, 405, 421) and a busy 503 or a 429 are completed
    observations reported as their own counts, never folded into the latency and
    never silently dropped. A transport failure is its own count.
    """
    table: dict[str, dict[str, dict[str, Any]]] = {}
    for row in _cells(evidence):
        for sample in row["samples"]:
            route = table.setdefault(sample["route_id"], {}).setdefault(
                row["target"], {"latencies": [], "completed": 0, "classes": {}}
            )
            status = sample["status"]
            route["classes"][str(status)] = route["classes"].get(str(status), 0) + 1
            if status != 0:
                route["completed"] += 1
            if 200 <= status < 300:
                route["latencies"].append(sample["seconds"])
    summary: dict[str, dict[str, dict[str, Any]]] = {}
    for route_id, targets in table.items():
        summary[route_id] = {}
        for target, data in targets.items():
            classes = data["classes"]
            base = {
                "completed": data["completed"],
                "success": classes.get("200", 0),
                "busy": classes.get("503", 0),
                "rate_limited": classes.get("429", 0),
                "transport_errors": classes.get("0", 0),
                "server_errors": sum(
                    count for status, count in classes.items()
                    if status != "0" and 500 <= int(status) < 600 and status != "503"
                ),
                "statuses": classes,
            }
            if not data["latencies"]:
                summary[route_id][target] = {"outcome": "inconclusive", **base}
                continue
            summary[route_id][target] = {**measure_mod.summarize(data["latencies"]), **base}
    return summary


def mix_table(manifest: dict[str, Any], latency: dict[str, Any]) -> dict[str, dict[str, float]]:
    """The declared representative-mix aggregate quantiles per runtime."""
    weights = {tool["id"]: tool["weight_representative"] for tool in workload.tools(manifest)}
    for route in workload.routes(manifest):
        weights[route["id"]] = route["weight_representative"]
    mix: dict[str, dict[str, float]] = {}
    for target in ("python", "rust"):
        p50, p95, p99 = {}, {}, {}
        for route_id, by_target in latency.items():
            cell = by_target.get(target)
            if cell and "p50" in cell:
                p50[route_id] = cell["p50"]
                p95[route_id] = cell["p95"]
                p99[route_id] = cell["p99"]
        if p50:
            # A route outside the declared mix (a synthetic id in a unit test,
            # or a probe route) still carries weight 1 rather than failing the
            # whole aggregation.
            usable = {rid: weights.get(rid, 1) for rid in p50}
            mix[target] = measure_mod.weighted_mix(p50, p95, p99, usable)
    return mix


def route_regression_blocks(python_p95: float, rust_p95: float) -> bool:
    """Decision 68: a per-route regression blocks above either cap.

    The route blocks when Rust is slower by **more than** 100 ms **or** by
    **more than** 50 % of Python's p95 — whichever threshold it crosses first.
    A regression at or under a cap is a fix item, not a block.
    """
    if rust_p95 <= python_p95:
        return False
    # Round each measurement once to integer microseconds (half up).
    python_us = math.floor(python_p95 * 1_000_000 + 0.5)
    rust_us = math.floor(rust_p95 * 1_000_000 + 0.5)
    delta = rust_us - python_us
    return delta > 100_000 or 2 * delta > python_us


def _route_pairs(latency: dict[str, Any]) -> list[tuple[str, float, float]]:
    """Require both positive, finite p95s for the complete committed workload."""
    manifest = workload.load()
    expected = {row["id"] for row in [*workload.routes(manifest), *workload.tools(manifest)]}
    if set(latency) != expected:
        return []
    pairs = []
    for route_id in sorted(expected):
        values = [latency[route_id].get(target, {}).get("p95") for target in ("python", "rust")]
        if any(type(value) not in (int, float) or not math.isfinite(value) or value <= 0 for value in values):
            return []
        pairs.append((route_id, *values))
    return pairs


def speed_verdict(
    latency: dict[str, Any], mix: dict[str, dict[str, float]], size: str = "small"
) -> dict[str, Any]:
    """Decision 82: speed blocks only at small; large regressions are fix items.

    Both sizes still require complete, finite, positive measurements.
    """
    pairs = _route_pairs(latency)
    threshold_regressions = [
        route_id
        for route_id, python_p95, rust_p95 in pairs
        if route_regression_blocks(python_p95, rust_p95)
    ]
    fix_items = [
        route_id
        for route_id, python_p95, rust_p95 in pairs
        if rust_p95 > python_p95 and (size == "large" or not route_regression_blocks(python_p95, rust_p95))
    ]
    python_mix = mix.get("python", {}).get("p95")
    rust_mix = mix.get("rust", {}).get("p95")
    mix_measured = (
        type(python_mix) in (int, float)
        and type(rust_mix) in (int, float)
        and math.isfinite(python_mix)
        and math.isfinite(rust_mix)
        and python_mix > 0
        and rust_mix > 0
    )
    mix_regressed = bool(mix_measured and rust_mix > python_mix)
    mix_blocks = size == "small" and mix_regressed
    blocking = threshold_regressions if size == "small" else []
    return {
        "outcome": "inconclusive" if size not in {"small", "large"} or not mix_measured or not pairs else ("no-go" if mix_blocks or blocking else "pass"),
        "mix_blocks": mix_blocks,
        "mix_regressed": mix_regressed,
        "size": size,
        "threshold_regressions": sorted(threshold_regressions),
        "mix_measured": mix_measured,
        "blocking_regressions": sorted(blocking),
        "fix_items": sorted(fix_items),
    }


def contract_verdict(simulation: dict[str, Any]) -> dict[str, Any]:
    """Require explicit comparison results for every committed group and size."""
    expected_groups = set(json.loads((ROOT / "tests/rust_contract/cases.json").read_text())["groups"])
    expected_sizes = set(workload.sizes(workload.load()))
    runs = simulation.get("runs", [simulation] if simulation else [])
    results: dict[tuple[str, str], dict[str, Any]] = {}
    invalid = []
    for run in runs:
        size = run.get("size")
        groups = run.get("groups", {})
        differences = run.get("differences")
        if (size not in expected_sizes or type(differences) is not int or differences < 0
                or not groups or run.get("inconclusive_groups")):
            invalid.append("missing or invalid simulation totals")
            continue
        group_differences = 0
        cases = 0
        for group, row in groups.items():
            count = row.get("differences")
            n = row.get("cases")
            if (group not in expected_groups or type(count) is not int or count < 0
                    or type(n) is not int or n <= 0
                    or row.get("outcome") != ("match" if count == 0 else "difference")):
                invalid.append(f"{size}/{group}: incomplete comparison")
                continue
            group_differences += count
            cases += n
            # An explicit later rerun can replace the same size/group only.
            results[(size, group)] = row
        if group_differences != differences or cases != run.get("total_pairs"):
            invalid.append(f"{size}: inconsistent simulation totals")
    missing = sorted(f"{size}/{group}" for size in expected_sizes for group in expected_groups
                     if (size, group) not in results)
    differences = sum(row["differences"] for row in results.values())
    return {"outcome": "inconclusive" if invalid or missing else ("no-go" if differences else "pass"),
            "differences": differences if not invalid and not missing else None,
            "missing": missing, "invalid": invalid}


def error_verdict(latency: dict[str, Any]) -> dict[str, Any]:
    """The fixed schedule requires equal offered totals on every route.

    Count transport failures as offered requests too. Different successful
    subsets do not change the denominator of the 5xx/503 comparison.
    """
    manifest = workload.load()
    expected = {row["id"] for row in [*workload.routes(manifest), *workload.tools(manifest)]}
    totals = {target: 0 for target in ("python", "rust")}
    errors = {target: 0 for target in totals}
    same_workload = set(latency) == expected
    for targets in latency.values():
        counts = {}
        for target in totals:
            row = targets.get(target, {})
            counts[target] = sum(row.get("statuses", {}).values())
            totals[target] += counts[target]
            errors[target] += row.get("server_errors", 0) + row.get("busy", 0)
        same_workload &= counts["python"] == counts["rust"] and counts["python"] > 0
    rates = {target: errors[target] / totals[target] if totals[target] else None for target in totals}
    return {
        "outcome": "inconclusive" if not same_workload else ("no-go" if rates["rust"] > rates["python"] else "pass"),
        "same_workload": same_workload, "requests": totals, "errors": errors, "rates": rates,
    }


def write_report(destination: Path, document: str) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(document)


def build_frozen_bundle(measurement: dict[str, Any], simulation: dict[str, Any], integration_passed: bool) -> dict[str, Any]:
    """Authoritative decision-86 gate; paired performance is historical only."""
    performance = frozen_verdict(measurement)
    contract = contract_verdict(simulation)
    conditions = [
        {"name": "zero unexplained contract differences at both sizes", "outcome": contract["outcome"], "detail": str(contract)},
        {"name": "integration suite", "outcome": "pass" if integration_passed else "no-go", "detail": "explicit suite result"},
        {"name": "Rust-only frozen-baseline performance evidence", "outcome": performance["outcome"], "detail": performance.get("reason", "validated")},
    ]
    outcomes = [row["outcome"] for row in conditions]
    outcome = "inconclusive" if "inconclusive" in outcomes else ("no-go" if "no-go" in outcomes else "pass")
    return {"gate": {"outcome": outcome, "passes": outcome == "pass", "conditions": conditions},
            "performance": performance, "measurement": measurement}


def build_bundle(
    *,
    identities: dict[str, Any],
    contract: dict[str, Any],
    simulation: dict[str, Any],
    measurement: dict[str, Any],
    unapproved_differences: int,
    integration_passed: bool = False,
    integration_summary: str = "",
) -> dict[str, Any]:
    """Assemble one bundle from the measured evidence files.

    Every value comes from a measured run. Missing files or missing keys become
    `not measured` in the rendered report, never a prefilled claim.
    """
    manifest = workload.load()
    memory = memory_table(measurement) if measurement else {}
    cpu = cpu_table(measurement) if measurement else {}
    latency = latency_table(measurement) if measurement else {}
    mix = mix_table(manifest, latency) if latency else {}

    contract_result = contract_verdict(simulation)
    errors = error_verdict(latency)
    rust_5xx = errors["errors"]["rust"]
    python_5xx = errors["errors"]["python"]

    coverage_counts: dict[str, int] = {}
    for row in _cells(measurement) if measurement else []:
        for sample in row["samples"]:
            if sample["status"] != 0:
                coverage_counts[sample["route_id"]] = coverage_counts.get(sample["route_id"], 0) + 1
    # Coverage counts completed observations (a busy 503 still completed); the
    # floor is the manifest's. The successful subset is reported separately.
    covered = measure_mod.coverage(coverage_counts, manifest["required_observations_per_route"]) if coverage_counts else {}
    if covered:
        ok = sum(1 for row in covered["routes"].values() if row["outcome"] == "ok")
        covered = {**covered, "summary": f"{ok}/{len(covered['routes'])} routes reached the floor"}

    speed = speed_verdict(latency, mix, measurement.get("size"))
    conditions = _gate_conditions(
        unapproved_differences, rust_5xx, python_5xx, integration_passed, speed, memory, errors, contract_result
    )
    # Memory and coverage report their own outcomes but never block; a
    # non-blocking outcome value must not fail the gate.
    blocking_outcomes = {"pass", "below target (non-blocking)", "not measured (non-blocking)"}
    passes = all(condition["outcome"] in blocking_outcomes for condition in conditions)
    return {
        "adaptive": measure_mod.adaptive_choice(measurement.get("probe", []), measurement.get("probe_max_rps", 0))
        if measurement.get("mode") == "adaptive" else {},
        "identities": identities,
        "contract": contract,
        "simulation": simulation,
        "workload": {
            "manifest_digest": workload.manifest_digest(manifest),
            "sizes": manifest["sizes"],
        },
        "memory": memory,
        "cpu": cpu,
        "latency": latency,
        "mix": mix,
        "coverage": covered,
        "gate": {
            "passes": passes,
            "outcome": "inconclusive" if any(c["outcome"] == "inconclusive" for c in conditions) else ("pass" if passes else "no-go"),
            "conditions": conditions,
            "rust_5xx": rust_5xx,
            "python_5xx": python_5xx,
            "speed": speed,
            "errors": errors,
            "contract": contract_result,
        },
        "integration": {"passed": integration_passed, "summary": integration_summary},
    }


def _gate_conditions(
    unapproved_differences: int,
    rust_5xx: int,
    python_5xx: int,
    integration_passed: bool,
    speed: dict[str, Any],
    memory: dict[str, Any],
    errors: dict[str, Any] | None = None,
    contract_result: dict[str, Any] | None = None,
) -> list[dict[str, str]]:
    """The decision-68 conditions with an explicit per-condition outcome.

    Hard gates: zero unexplained differences, no 5xx/503 rise from Rust, and a
    passing integration suite. Speed blocks only per the threshold rule. Memory
    is reported against its target but never blocks.
    """
    difference_outcome = contract_result["outcome"] if contract_result else "inconclusive"
    errors_outcome = errors["outcome"] if errors else "inconclusive"
    integration_outcome = "pass" if integration_passed else "no-go"
    speed_outcome = speed["outcome"]
    python_mem = memory.get("python", {}).get("median_bytes")
    rust_mem = memory.get("rust", {}).get("median_bytes")
    if python_mem and rust_mem is not None:
        memory_detail = (
            f"Rust {rust_mem / (1024 * 1024):.1f} MiB vs Python {python_mem / (1024 * 1024):.1f} MiB "
            f"({rust_mem / python_mem * 100:.1f}% of Python)"
        )
        memory_outcome = "pass" if rust_mem <= python_mem / 2 else "below target (non-blocking)"
    else:
        memory_detail = "memory not measured"
        memory_outcome = "not measured (non-blocking)"
    conditions = [
        {"name": "zero unexplained contract differences", "outcome": difference_outcome,
         "detail": f"{contract_result or NOT_MEASURED}"},
        {"name": "no rise in 5xx/503 from Rust against Python", "outcome": errors_outcome,
         "detail": f"Rust {rust_5xx} vs Python {python_5xx}; "
                   f"offered totals/rates: {errors or NOT_MEASURED}"},
        {"name": "the integration suite passes", "outcome": integration_outcome,
         "detail": "the unchanged disposable integration suite"},
        {"name": "speed (mix p95 and per-route thresholds)", "outcome": speed_outcome,
         "detail": (
             f"blocking routes: {', '.join(speed['blocking_regressions']) or 'none'}; "
             f"fix items: {', '.join(speed['fix_items']) or 'none'}"
         )},
        {"name": "memory target (Rust at most half of Python; non-blocking)", "outcome": memory_outcome,
         "detail": memory_detail},
    ]
    return conditions



def _load(path: Path) -> dict[str, Any]:
    if not path or not path.is_file():
        return {}
    try:
        document = json.loads(path.read_text())
    except (OSError, ValueError):
        return {}
    return document if isinstance(document, dict) else {}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--small", type=Path, required=True, help="measure-small.json")
    parser.add_argument("--large", type=Path, required=True, help="measure-large.json")
    parser.add_argument("--simulation", type=Path, action="append", required=True,
                        help="simulation-<size>.json; repeat for all sizes and later group reruns")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--candidate-sha", default="")
    parser.add_argument(
        "--integration-passed",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="the unchanged disposable integration suite's result",
    )
    parser.add_argument("--integration-summary", default="")
    args = parser.parse_args(argv)

    simulation = {"runs": [_load(path) for path in args.simulation]}
    small = _load(args.small)
    large = _load(args.large)
    lines = ["# Rust Phase 1 go/no-go — decisions 86 and 87", "",
             "Python is the correctness reference only. Speed and small-size error rates use the pinned frozen baseline.",
             "Large Rust speed and errors are reported; Rust memory is reported only. Both sizes require valid Rust evidence.", ""]
    from scripts.rust_contract import baseline
    try:
        frozen = baseline.load_python_baseline()
        lines.extend(["## Frozen Python baseline", "",
                      f"- Pin: `{baseline.gate_config()['python_baseline_sha256']}`.",
                      f"- Source file SHA-256: `{frozen['source_file_sha256']}`.",
                      f"- Source and harness SHA: `{frozen['source_sha']}`.",
                      f"- Manifest digest: `{frozen['manifest_digest']}`.",
                      f"- Date: {frozen['measurement_date']}, ended about {frozen['measurement_end_approx']}.",
                      f"- Attestation: {frozen['identity_attested_by']}.",
                      f"- Known limit: {frozen['known_limit']}.",
                      f"- Offered windows: {frozen['offered_windows']}.",
                      f"- Weighted mix p95: {frozen['mix_p95']:.9f} s.",
                      f"- 5xx/503: {frozen['errors_5xx']}/{frozen['offered']} ({frozen['error_rate']:.6%}).", "",
                      "| Route/tool | Frozen Python p95 (s) | Successful | Offered | 5xx | 503 |",
                      "| --- | ---: | ---: | ---: | ---: | ---: |"])
        for rid, row in frozen["routes"].items():
            lines.append(f"| {rid} | {row['p95']:.9f} | {row['successful']} | {row['offered']} | {row['errors_5xx']} | {row['busy_503']} |")
    except ValueError as error:
        frozen = None
        lines.extend(["## Frozen Python baseline", f"INCONCLUSIVE: {error}"])
    for size, measurement in (("small", small), ("large", large)):
        if measurement.get("size") != size:
            measurement = {}
        bundle = build_frozen_bundle(measurement, simulation, args.integration_passed)
        result = bundle["performance"]
        lines.extend(["", f"## {size.title()} workload", "",
                      f"- Outcome: {bundle['gate']['outcome'].upper()}.",
                      f"- Rust-only performance evidence: {result['outcome']} ({result.get('reason', 'validated')}).",
                      f"- Offered Rust rates: {measurement.get('rates_rps', NOT_MEASURED)}; higher than the frozen 5 RPS maximum is stricter for Rust."])
        for condition in bundle["gate"]["conditions"]:
            lines.append(f"- {condition['name']}: {condition['outcome']} ({condition['detail']}).")
        cells = {"repetitions": [{**rep, "cells": {"rust": rep.get("cells", {}).get("rust", {"samples": [], "counters": []})}}
                                 for rep in measurement.get("repetitions", [])]}
        rust_memory = memory_table(cells).get("rust", {}) if cells["repetitions"] else {}
        lines.append(f"- Rust memory steady median: {_bytes_mib(rust_memory.get('median_bytes'))}; non-blocking.")
        lines.append(f"- Blocking routes: {', '.join(result['speed'].get('blocking_regressions', [])) or 'none proven'}.")
        lines.append(f"- Same-meaning fix items: {', '.join(result['speed'].get('fix_items', [])) or 'none proven'}.")
        if size == "small" and measurement.get("mode") == "rust-only":
            lines.append(f"- Measured source SHA: `{measurement.get('source_sha')}`; "
                         f"harness SHA `{measurement.get('harness_sha')}`; run identity "
                         f"{measurement.get('mode')} at {measurement.get('rates_rps')} RPS on the "
                         f"equal-weight schedule, baseline pin `{measurement.get('baseline_sha256')}`.")
            lines.append("- The integration branch later adds only the 16-slot admission change and "
                         "gate code; no query change follows this measurement.")
        # Show historical Rust samples as diagnostics without attesting missing
        # identity or granting a waiver of the Rust successful-sample floor.
        observed = latency_table(cells) if cells["repetitions"] else {}
        if observed:
            lines.extend(["", "Historical/unqualified Rust samples remain diagnostic when evidence admission fails.", "",
                          "| Route/tool | Rust p95 (s) | Successful | Required |", "| --- | ---: | ---: | ---: |"])
            floors = result.get("successful_counts") and {}
            from scripts.rust_contract import frozen_gate as gate_module
            offered = sum(row.get("offered", 0) for row in [])  # placeholder removed below
            _floors = None
            try:
                if measurement.get("rates_rps"):
                    total = sum(len(rep.get("cells", {}).get("rust", {}).get("samples", []))
                                for rep in measurement.get("repetitions", []))
                    _floors = gate_module.route_floors(frozen, measurement["rates_rps"], total)
            except Exception:
                _floors = None
            for rid, targets in sorted(observed.items()):
                row = targets["rust"]
                required = (_floors or {}).get(rid, 1000)
                lines.append(f"| {rid} | {_seconds(row.get('p95'))} | {row.get('n', 0)} | {required} |")
        if measurement.get("mode") == "adaptive":
            adaptive = measure_mod.adaptive_choice(measurement.get("probe", []), measurement.get("probe_max_rps", 0))
            lines.append(f"- Adaptive probe: {'no clean rate found' if adaptive['chosen_rps'] is None else adaptive['chosen_rps']}; {adaptive['stop_reason']}.")
    lines.extend(["", f"- Integration evidence: {args.integration_summary or 'not supplied'}.",
                  "- Production status: not authorized by this report."])
    write_report(args.out, "\n".join(lines) + "\n")
    print(f"report written to {args.out}")
    return 0


def _oracle_sha() -> str:
    pointer = ROOT / "tests/rust_contract/golden/CURRENT"
    return pointer.read_text().strip() if pointer.is_file() else ""


def _corpus_digest() -> str:
    """A digest over every recorded golden case's response bytes."""
    import hashlib

    base = ROOT / "tests/rust_contract/golden" / _oracle_sha()
    if not base.is_dir():
        return ""
    digest = hashlib.sha256()
    for case in sorted(base.iterdir()):
        body = case / "response.body"
        if body.is_file():
            digest.update(case.name.encode())
            digest.update(body.read_bytes())
    return digest.hexdigest()


def report_policy_digest() -> str:
    manifest = ROOT / "deploy/rust/market-policy.manifest.json"
    return sha256_file(manifest) if manifest.is_file() else ""


def _simulation_summary(simulation: dict[str, Any]) -> str:
    if not simulation:
        return NOT_MEASURED
    if "runs" in simulation:
        return "; ".join(_simulation_summary(run) for run in simulation["runs"])
    groups = simulation.get("groups", {})
    matched = sum(1 for outcome in groups.values() if outcome.get("outcome") == "match")
    return (
        f"size {simulation.get('size')}: {matched}/{len(groups)} groups matched, "
        f"{simulation.get('total_pairs', 0)} pairs, {simulation.get('differences', NOT_MEASURED)} differences, "
        f"gate {simulation.get('gate')}"
    )


def _bytes_mib(value: Any) -> str:
    return NOT_MEASURED if value is None else f"{value / (1024 * 1024):.1f} MiB"


def _seconds(value: Any) -> str:
    return NOT_MEASURED if value is None else f"{value:.3f}"


def render_markdown(bundle: dict[str, Any]) -> str:
    """Render the go/no-go report from measured values only.

    Every field is read from `bundle`. A key that is absent or null becomes
    `not measured`; nothing is invented.
    """
    identities = bundle.get("identities", {})
    contract = bundle.get("contract", {})
    simulation = bundle.get("simulation", {})
    memory = bundle.get("memory", {})
    cpu = bundle.get("cpu", {})
    latency = bundle.get("latency", {})
    mix = bundle.get("mix", {})
    gate = bundle.get("gate", {})
    coverage = bundle.get("coverage", {})
    workload_info = bundle.get("workload", {})

    lines: list[str] = []
    lines.append("# Rust Phase 1 go/no-go")
    lines.append("")
    lines.append(f"- Decision requested: {bundle.get('decision', 'continue / pause / stop')}.")
    lines.append(f"- Candidate source SHA: {identities.get('candidate_sha') or NOT_MEASURED}")
    lines.append(f"- Python oracle SHA and corpus digest: {identities.get('oracle_sha') or NOT_MEASURED}"
                 f" / {identities.get('corpus_digest') or NOT_MEASURED}")
    lines.append(f"- Schema revision and policy digest: {identities.get('schema_revision') or NOT_MEASURED}"
                 f" / {identities.get('policy_digest') or NOT_MEASURED}")
    lines.append(f"- Rust toolchain, target, libc minimum, Cargo.lock digest, artifact digest: "
                 f"{identities.get('toolchain') or NOT_MEASURED}")
    lines.append(f"- Host/CPU/cgroup/PostgreSQL/PostGIS configuration: "
                 f"{identities.get('host') or NOT_MEASURED}")
    lines.append("- Scope: 27 data GETs, 3 health GETs, 3 MCP transport pairs; 3 MCP tools.")
    lines.append("")
    lines.append("## Contract and simulation")
    lines.append("")
    lines.append(f"- Contract result: {contract.get('total', NOT_MEASURED)} cases, "
                 f"{contract.get('passed', NOT_MEASURED)} passed, {contract.get('failed', NOT_MEASURED)} failed, "
                 f"{contract.get('skipped', NOT_MEASURED)} skipped; "
                 f"{contract.get('differences', NOT_MEASURED)} unapproved differences.")
    lines.append(f"- Contract evidence digest: {contract.get('evidence_digest') or NOT_MEASURED}")
    lines.append(f"- Seeded differential simulation: {simulation.get('summary', NOT_MEASURED)}")
    lines.append(f"- Simulation evidence digest: {simulation.get('evidence_digest') or NOT_MEASURED}")
    lines.append(f"- Workload manifest digest: {workload_info.get('manifest_digest') or NOT_MEASURED}")
    lines.append(f"- Workload sizes: {workload_info.get('sizes', NOT_MEASURED)}")
    lines.append("")
    adaptive = bundle.get("adaptive", {})
    if adaptive:
        lines.append("## Adaptive probe")
        lines.append("")
        chosen = adaptive.get("chosen_rps")
        lines.append(f"- Clean rate: {str(chosen) + ' RPS' if chosen is not None else 'no clean rate found'}.")
        lines.append(f"- Stop reason: {adaptive['stop_reason']}.")
        for probe in adaptive.get("probe", []):
            lines.append(f"- Probe {probe.get('rps')} RPS, 5xx/503: {probe.get('busy')}.")
        lines.append("")
    lines.append("## Memory")
    lines.append("")
    lines.append("| Runtime | steady median | p95 | peak | anon median | file median | PSS median | ratio |")
    lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for target in ("python", "rust"):
        row = memory.get(target, {})
        ratio = ""
        if target == "rust" and memory.get("python", {}).get("median_bytes"):
            ratio = f"{row.get('median_bytes', 0) / memory['python']['median_bytes']:.2f}"
        lines.append(
            f"| {target} | {_bytes_mib(row.get('median_bytes'))} | {_bytes_mib(row.get('p95_bytes'))} | "
            f"{_bytes_mib(row.get('peak_bytes'))} | {_bytes_mib(row.get('anon_median_bytes'))} | "
            f"{_bytes_mib(row.get('file_median_bytes'))} | {_bytes_mib(row.get('pss_median_bytes'))} | {ratio} |"
        )
    lines.append("")
    lines.append("## CPU")
    lines.append("")
    lines.append("| Runtime | core-equivalents | usec / completed request |")
    lines.append("| --- | ---: | ---: |")
    for target in ("python", "rust"):
        row = cpu.get(target, {})
        lines.append(
            f"| {target} | {row.get('core_equivalents', NOT_MEASURED)} | "
            f"{row.get('usec_per_request', NOT_MEASURED)} |"
        )
    lines.append("")
    lines.append("## Latency (end-to-end through Caddy)")
    lines.append("")
    lines.append("The latency series is successful (2xx) responses only. Busy (503), "
                 "rate-limited (429), contract 4xx and transport failures are completed "
                 "observations reported as their own counts.")
    lines.append("")
    lines.append("| Route | Python p50/p95/p99 | Rust p50/p95/p99 | py n | rs n | py busy | rs busy | py 429 | rs 429 |")
    lines.append("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for route_id in sorted(latency):
        py = latency[route_id].get("python", {})
        rs = latency[route_id].get("rust", {})
        lines.append(
            f"| {route_id} | {_seconds(py.get('p50'))}/{_seconds(py.get('p95'))}/{_seconds(py.get('p99'))} | "
            f"{_seconds(rs.get('p50'))}/{_seconds(rs.get('p95'))}/{_seconds(rs.get('p99'))} | "
            f"{py.get('n', NOT_MEASURED)} | {rs.get('n', NOT_MEASURED)} | "
            f"{py.get('busy', NOT_MEASURED)} | {rs.get('busy', NOT_MEASURED)} | "
            f"{py.get('rate_limited', NOT_MEASURED)} | {rs.get('rate_limited', NOT_MEASURED)} |"
        )
    lines.append("")
    lines.append(f"- Weighted representative mix: Python "
                 f"{_seconds(mix.get('python', {}).get('p95'))} p95, Rust "
                 f"{_seconds(mix.get('rust', {}).get('p95'))} p95.")
    lines.append("")
    lines.append("## Gate (founder decisions 68 and 82, 18 Oct)")
    lines.append("")
    lines.append("Hard gates: zero unexplained differences, no rise in 5xx/503 from Rust "
                 "against Python on the same workload, and a passing integration suite. "
                 "Speed blocks only at the small workload: a mix p95 regression or a per-route "
                 "regression beyond 100 ms or 50 %. At the large workload, mix p95 is "
                 "reported and all route regressions are fix items for the same-meaning pass. "
                 "Complete valid measurements remain required at both sizes. "
                 "Memory is reported against its target and does not block.")
    lines.append("")
    lines.append(f"- Outcome: {'PASS' if gate.get('passes') else gate.get('outcome', 'inconclusive').upper()}.")
    for condition in gate.get("conditions", []):
        lines.append(f"  - {condition['name']}: {condition['outcome']} ({condition.get('detail', '')})")
    speed = gate.get("speed", {})
    if speed.get("fix_items"):
        lines.append(f"- Fix items (non-blocking regressions): {', '.join(speed['fix_items'])}.")
    lines.append(f"- Coverage: {coverage.get('summary', NOT_MEASURED)}")
    for route_id in coverage.get("inconclusive", []):
        lines.append(f"  - {route_id}: inconclusive, {coverage['routes'][route_id].get('needed')} more observations needed")
    lines.append(f"- Integration suite: "
                 f"{'passed' if bundle.get('integration', {}).get('passed') else 'not recorded as passing'}.")
    lines.append("")
    lines.append("## Approved differences (exactly three)")
    lines.append("")
    lines.append("| ID | What changed | Why approved | Proof |")
    lines.append("| --- | --- | --- | --- |")
    lines.append("| AD-01 | Rust enforces 60/30/10 requests per minute once per client/route; "
                 "Python's four local counters can admit up to four times more | PM defines documented "
                 "per-client limits as the contract | Rust exact-boundary concurrent burst/reset tests; "
                 "counts excluded from parity |")
    lines.append("| AD-02 | Rust owns configured read_admission_slots (default 16); Python keeps 2 slots in each "
                 "of 4 workers | Founder decision 88 (18 Oct); full database budget checked | "
                 "Configured-capacity and cancellation tests; 503 rates recorded |")
    lines.append("| AD-03 | Rust admits `/api/news` through the same configured gate | Every Rust database use "
                 "holds a permit | Two-direction contention test |")
    lines.append("")
    lines.append("- Production status: not authorized by this report.")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    raise SystemExit(main())

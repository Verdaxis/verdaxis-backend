"""Decision 86 Rust-only performance evidence, checked against frozen Python."""
from __future__ import annotations

import math
import re
from datetime import date
from typing import Any

from scripts.rust_contract import baseline, measure, workload


def validate_identity(evidence: dict[str, Any], reference: dict[str, Any]) -> None:
    config = baseline.gate_config()
    if evidence.get("mode") != "rust-only" or evidence.get("size") not in {"small", "large"}:
        raise ValueError("missing Rust-only workload identity")
    for key in ("source_sha", "harness_sha"):
        if not re.fullmatch("[0-9a-f]{40}", evidence.get(key, "")):
            raise ValueError(f"missing or invalid {key}")
    date.fromisoformat(evidence.get("measurement_date", ""))
    if evidence.get("manifest_digest") != reference["manifest_digest"]:
        raise ValueError("measurement workload differs from frozen baseline")
    if evidence.get("baseline_sha256") != config["python_baseline_sha256"]:
        raise ValueError("measurement baseline pin differs")
    rates = evidence.get("rates_rps", [])
    minimum = max(window["rps"] for window in reference["offered_windows"])
    if not rates or any(type(rate) not in (int, float) or not math.isfinite(rate) or rate < minimum for rate in rates):
        raise ValueError("Rust offered rate must be at least the frozen maximum rate")
    if evidence.get("weight_key") != "weight_baseline":
        raise ValueError("Rust schedule must use the frozen equal-weight workload")


def frozen_verdict(evidence: dict[str, Any]) -> dict[str, Any]:
    # Local import avoids a report/measure import cycle while sharing the exact
    # percentile and threshold logic used by historical report rendering.
    from scripts.rust_contract import report

    try:
        reference = baseline.load_python_baseline()
        validate_identity(evidence, reference)
        expected = set(reference["routes"])
        samples: dict[str, list[float]] = {rid: [] for rid in expected}
        offered = errors = 0
        declared_rates = evidence["rates_rps"]
        for repetition in evidence.get("repetitions", []):
            if set(repetition.get("cells", {})) != {"rust"}:
                raise ValueError("Rust-only measurement must not contain Python cells")
            for sample in repetition["cells"]["rust"]["samples"]:
                if sample["route_id"] not in expected or sample.get("rps") not in declared_rates:
                    raise ValueError("unexpected route or offered rate")
                seconds, status = sample["seconds"], sample["status"]
                if type(seconds) not in (int, float) or not math.isfinite(seconds) or seconds <= 0:
                    raise ValueError("invalid Rust latency")
                if type(status) is not int or not (status == 0 or 100 <= status < 600):
                    raise ValueError("invalid response status")
                offered += 1
                errors += 500 <= status < 600
                if 200 <= status < 300:
                    samples[sample["route_id"]].append(seconds)
        floor = baseline.gate_config()["rust_successful_samples_per_route"]
        if type(floor) is not int or floor < 1000:
            raise ValueError("Rust successful sample floor must be at least 1000")
        counts = {rid: len(values) for rid, values in samples.items()}
        if any(count < floor for count in counts.values()):
            raise ValueError(f"Rust successful sample floor not reached: {counts}")
        latency = {rid: {"python": {"p95": reference["routes"][rid]["p95"]},
                         "rust": {"p95": measure.summarize(values)["p95"]}}
                   for rid, values in samples.items()}
        manifest = workload.load()
        weights = {r["id"]: r["weight_representative"] for r in workload.routes(manifest) + workload.tools(manifest)}
        rust_mix = sum(weights[rid] * row["rust"]["p95"] for rid, row in latency.items()) / sum(weights.values())
        speed = report.speed_verdict(latency, {"python": {"p95": reference["mix_p95"]}, "rust": {"p95": rust_mix}}, evidence["size"])
        rate_blocks = errors * reference["offered"] > reference["errors_5xx"] * offered
        error_result = {"outcome": "no-go" if rate_blocks and evidence["size"] == "small" else "pass",
                        "rust_errors": errors, "rust_offered": offered, "rust_rate": errors / offered,
                        "baseline_rate": reference["error_rate"], "reported_only": evidence["size"] == "large"}
        return {"outcome": "no-go" if speed["outcome"] == "no-go" or error_result["outcome"] == "no-go" else "pass",
                "speed": speed, "errors": error_result, "rust_mix_p95": rust_mix,
                "successful_counts": counts, "baseline": reference}
    except (ValueError, KeyError, TypeError, OSError) as error:
        return {"outcome": "inconclusive", "reason": str(error),
                "speed": {"outcome": "inconclusive", "blocking_regressions": [], "fix_items": []},
                "errors": {"outcome": "inconclusive"}}

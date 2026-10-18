"""Decisions 86/87: a pinned Python baseline, never a new Python speed run."""
import copy
import hashlib
import json

import pytest

from scripts.rust_contract import baseline as report
from scripts.rust_contract import workload


def test_baseline_is_pinned_and_complete():
    baseline = report.load_python_baseline()
    expected = {r["id"] for r in workload.routes(workload.load()) + workload.tools(workload.load())}
    assert set(baseline["routes"]) == expected
    assert baseline["identity_attested_by"] == "PM, tracker decision 87"
    assert baseline["routes"]["R27"]["successful"] == 150
    assert baseline["known_limit"] == "below the 1,000-sample floor; frozen by founder decision 87"


@pytest.mark.parametrize("defect", ["digest", "missing_route", "missing_pin", "invalid_p95"])
def test_baseline_refuses_invalid_evidence(tmp_path, defect):
    baseline = copy.deepcopy(report.load_python_baseline())
    if defect == "missing_route":
        baseline["routes"].pop("R27")
    if defect == "invalid_p95":
        baseline["routes"]["R27"]["p95"] = -1
    data = json.dumps(baseline).encode()
    path = tmp_path / "baseline.json"
    path.write_bytes(data)
    pin = hashlib.sha256(data).hexdigest()
    if defect == "digest":
        pin = "0" * 64
    if defect == "missing_pin":
        pin = ""
    with pytest.raises(ValueError):
        report.load_python_baseline(path, pin)


def rust_measurement():
    base = report.load_python_baseline()
    config = report.gate_config()
    return {"size": "small", "mode": "rust-only", "source_sha": "a" * 40, "harness_sha": "a" * 40,
            "manifest_digest": base["manifest_digest"], "baseline_sha256": config["python_baseline_sha256"],
            "measurement_date": "2026-10-18", "rates_rps": [10.0], "weight_key": "weight_baseline",
            "repetitions": [{"index": 0, "cells": {"rust": {"counters": [], "samples": [
                {"route_id": rid, "seconds": row["p95"] / 2, "status": 200, "rps": 10.0}
                for rid, row in base["routes"].items() for _ in range(1000)]}}}]}


def verdict(evidence):
    from scripts.rust_contract import report as reports
    return reports.frozen_verdict(evidence)


def test_frozen_rust_within_limits_passes_without_python():
    assert verdict(rust_measurement())["outcome"] == "pass"


@pytest.mark.parametrize("python,rust,blocks", [(0.300, 0.400, False), (0.300, 0.400001, True),
                                               (0.180, 0.270, False), (0.180, 0.270001, True)])
def test_frozen_threshold_boundaries(python, rust, blocks):
    from scripts.rust_contract.report import route_regression_blocks
    assert route_regression_blocks(python, rust) is blocks


@pytest.mark.parametrize("defect", ["floor", "identity", "manifest", "pin", "missing_route", "invalid_p95", "lower_rate"])
def test_rust_evidence_fail_closed(defect):
    evidence = rust_measurement()
    samples = evidence["repetitions"][0]["cells"]["rust"]["samples"]
    if defect == "floor": samples.pop()
    elif defect == "identity": evidence.pop("harness_sha")
    elif defect == "manifest": evidence["manifest_digest"] = "0" * 64
    elif defect == "pin": evidence["baseline_sha256"] = "0" * 64
    elif defect == "missing_route": samples[:] = [s for s in samples if s["route_id"] != "R27"]
    elif defect == "invalid_p95": samples[0]["seconds"] = float("nan")
    else: evidence["rates_rps"] = [1.0]
    assert verdict(evidence)["outcome"] == "inconclusive"


def test_rust_error_rate_above_baseline_blocks():
    evidence = rust_measurement()
    samples = evidence["repetitions"][0]["cells"]["rust"]["samples"]
    samples.extend({"route_id": "R27", "seconds": 0.1, "status": 503, "rps": 10.0} for _ in range(2000))
    result = verdict(evidence)
    assert result["outcome"] == "no-go"
    assert result["errors"]["outcome"] == "no-go"


def test_frozen_speed_and_mix_block_only_small():
    evidence = rust_measurement()
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        sample["seconds"] = 5.0
    small = verdict(evidence)
    assert small["outcome"] == "no-go"
    assert small["speed"]["mix_blocks"]
    evidence["size"] = "large"
    large = verdict(evidence)
    assert large["outcome"] == "pass"
    assert "R27" in large["speed"]["fix_items"]


def test_unpinned_baseline_is_inconclusive(monkeypatch):
    evidence = rust_measurement()
    monkeypatch.setattr(report, "gate_config", lambda: {"python_baseline_path": "tests/rust_contract/python-baseline-small.json", "python_baseline_sha256": ""})
    assert verdict(evidence)["outcome"] == "inconclusive"


def test_authoritative_report_never_uses_live_python_performance():
    from scripts.rust_contract import report as reports
    evidence = rust_measurement()
    # A historical paired file must not silently become a valid Rust-only run.
    evidence["repetitions"][0]["cells"]["python"] = {"samples": [], "counters": []}
    bundle = reports.build_frozen_bundle(evidence, {}, True)
    assert bundle["gate"]["outcome"] == "inconclusive"


def test_rust_only_measurement_identity_and_targets():
    from scripts.rust_contract.measure import Measurement
    run = Measurement("small", output_dir=__import__("pathlib").Path("work/test"), warmup_seconds=0,
                      windows=[(3000, 10)], repetitions=1, rust_only=True)
    identity = run.measurement_identity()
    assert run.measurement_targets(1, 2) == [("rust", 2)]
    assert identity["mode"] == "rust-only"
    assert identity["baseline_sha256"] == report.gate_config()["python_baseline_sha256"]
    assert identity["manifest_digest"] == report.load_python_baseline()["manifest_digest"]
    assert identity["rates_rps"] == [10]


def test_cli_default_uses_frozen_gate_for_historical_evidence(tmp_path):
    from scripts.rust_contract import report as reports
    old = tmp_path / "historical.json"
    old.write_text(json.dumps({"size": "small", "repetitions": []}))
    sim = tmp_path / "sim.json"
    sim.write_text("{}")
    out = tmp_path / "report.md"
    reports.main(["--small", str(old), "--large", str(old), "--simulation", str(sim), "--out", str(out)])
    text = out.read_text()
    assert "decisions 86 and 87" in text
    assert "Frozen Python baseline" in text
    assert "Rust-only" in text
    assert "INCONCLUSIVE" in text
    assert "| python |" not in text


def test_floor_cannot_be_lowered_in_gate_configuration(monkeypatch):
    evidence = rust_measurement()
    config = report.gate_config()
    config["rust_successful_samples_per_route"] = 0
    monkeypatch.setattr(report, "gate_config", lambda: config)
    assert verdict(evidence)["outcome"] == "inconclusive"


def test_actual_frozen_r27_relative_boundary():
    evidence = rust_measurement()
    base = report.load_python_baseline()
    python_us = __import__("math").floor(base["routes"]["R27"]["p95"] * 1_000_000 + 0.5)
    limit = (python_us + python_us // 2) / 1_000_000
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        if sample["route_id"] == "R27": sample["seconds"] = limit
    assert verdict(evidence)["outcome"] == "pass"
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        if sample["route_id"] == "R27": sample["seconds"] += 0.000001
    assert verdict(evidence)["outcome"] == "no-go"


def test_measurement_defaults_to_rust_only():
    from pathlib import Path

    from scripts.rust_contract.measure import Measurement
    run = Measurement("small", output_dir=Path("work/test"), warmup_seconds=0,
                      windows=[(3000, 10)], repetitions=1)
    assert run.measurement_targets(1, 2) == [("rust", 2)]


# ------------------------------------------------- decision 96 rate-limited floors


def limited_measurement(route_counts: dict | None = None, offered_per_route: int = 1200):
    """A Rust-only 3,600 s run at 10 RPS (30 routes x 1,200 offered).

    `route_counts` overrides a route's successful 2xx count; the rest of that
    route's offers are recorded as rate-limited 429 responses, exactly as the
    real measurement records them. The offered total stays 36,000, so the
    gate's derived window is exactly 3,600 s.
    """
    base = report.load_python_baseline()
    config = report.gate_config()
    route_counts = route_counts or {}
    samples = []
    for rid, row in base["routes"].items():
        successes = route_counts.get(rid, 1000)
        successes = min(successes, offered_per_route)
        samples.extend({"route_id": rid, "seconds": row["p95"] / 2, "status": 200, "rps": 10.0}
                       for _ in range(successes))
        samples.extend({"route_id": rid, "seconds": row["p95"] / 2, "status": 429, "rps": 10.0}
                       for _ in range(offered_per_route - successes))
    return {"size": "small", "mode": "rust-only", "source_sha": "a" * 40, "harness_sha": "a" * 40,
            "manifest_digest": base["manifest_digest"], "baseline_sha256": config["python_baseline_sha256"],
            "measurement_date": "2026-10-18", "rates_rps": [10.0], "weight_key": "weight_baseline",
            "repetitions": [{"index": 0, "cells": {"rust": {"counters": [], "samples": samples}}}]}


def rate_limited_route():
    """The route the committed limits table names (R14, 10 per minute)."""
    limits = report.gate_config()["rust_route_limits_per_minute"]
    assert set(limits) <= set(report.load_python_baseline()["routes"])
    rid = "R14"  # the 10-per-minute export route the decision names
    assert limits[rid] == 10
    return rid


@pytest.mark.parametrize("size,provided,expected", [
    ("large", False, "pass"),
    ("small", False, "inconclusive"),
    ("large", True, "inconclusive"),
])
def test_optional_performance_evidence_preserves_admission(size, provided, expected):
    from scripts.rust_contract.frozen_gate import performance_evidence
    result = performance_evidence(size, {} if provided else None)
    assert result["outcome"] == expected
    if size == "large" and not provided:
        assert result["reason"] == "large-size speed not measured (decisions 82 and 96)"


def test_optional_large_present_regression_is_a_fix_item():
    from scripts.rust_contract.frozen_gate import performance_evidence
    evidence = rust_measurement()
    evidence["size"] = "large"
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        sample["seconds"] = 5.0
    result = performance_evidence("large", evidence)
    assert result["outcome"] == "pass"
    assert "R27" in result["speed"]["fix_items"]


@pytest.mark.parametrize("content", ["{}", "not json", '{"size":"small"}'])
def test_cli_does_not_ignore_invalid_supplied_large_evidence(tmp_path, content):
    from scripts.rust_contract import report as reports
    small = tmp_path / "small.json"
    small.write_text(json.dumps(rust_measurement()))
    large = tmp_path / "large.json"
    large.write_text(content)
    sim = tmp_path / "sim.json"
    sim.write_text("{}")
    out = tmp_path / "report.md"
    reports.main(["--small", str(small), "--large", str(large), "--simulation", str(sim), "--out", str(out)])
    text = out.read_text().split("## Large workload")[1]
    assert "Rust-only performance evidence: inconclusive" in text
    assert "large-size speed not measured" not in text


@pytest.mark.parametrize("complete_contract", [False, True])
def test_cli_allows_omitted_large_evidence(tmp_path, complete_contract):
    from scripts.rust_contract import report as reports
    small = tmp_path / "small.json"
    small.write_text(json.dumps(rust_measurement()))
    sim = tmp_path / "sim.json"
    groups = workload.ROOT / "tests/rust_contract/cases.json"
    names = json.loads(groups.read_text())["groups"]
    simulations = [
        {"size": size, "groups": {name: {"outcome": "match", "cases": 1, "differences": 0} for name in names},
         "total_pairs": len(names), "differences": 0, "inconclusive_groups": []}
        for size in ("small", "large")
    ] if complete_contract else [{}]
    sim.write_text(json.dumps(simulations[0]))
    second = tmp_path / "sim-large.json"
    second.write_text(json.dumps(simulations[-1]))
    out = tmp_path / "report.md"
    reports.main(["--small", str(small), "--simulation", str(sim), "--simulation", str(second),
                  "--integration-passed", "--out", str(out)])
    assert "large-size speed not measured (decisions 82 and 96)" in out.read_text()
    expected = "GO for the founder's review" if complete_contract else "INCONCLUSIVE"
    assert f"Overall: {expected}" in out.read_text()


def test_rate_limited_route_reaches_its_written_floor():
    # R14's written limit allows 600 successes in 3,600 s; 95% of that is 570,
    # and the real decision-87 run recorded exactly 596.
    assert verdict(limited_measurement({"R14": 596}))["outcome"] == "pass"


def test_rate_limited_route_below_its_written_floor_fails():
    # 569 successes is below the 570 floor (95% of the 600 the limit allows).
    assert verdict(limited_measurement({"R14": 569}))["outcome"] == "inconclusive"


def test_unlimited_route_at_999_still_fails():
    # A route with no written limit keeps the full 1,000 floor.
    assert verdict(limited_measurement({"R27": 999}))["outcome"] == "inconclusive"


def test_missing_limit_entry_is_inconclusive(monkeypatch):
    # The limits table must name real routes only; a non-positive limit is a defect.
    rid = rate_limited_route()
    evidence = limited_measurement({rid: 596})
    config = report.gate_config()
    broken = {**config, "rust_route_limits_per_minute": {rid: 0}}
    monkeypatch.setattr(report, "gate_config", lambda: broken)
    assert verdict(evidence)["outcome"] == "inconclusive"

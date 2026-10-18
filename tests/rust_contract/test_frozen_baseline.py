"""Decisions 86/87: a pinned Python baseline, never a new Python speed run."""
import copy
import hashlib
import json

import pytest

from scripts.rust_contract import baseline as report, workload


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

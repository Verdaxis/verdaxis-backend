"""Successor gate keeps decision 87 and does not alter byte-cutover policy."""

import copy
import hashlib
import json

import pytest

from scripts.rust_contract import baseline, frozen_gate
from tests.rust_contract.test_frozen_baseline import rust_measurement


def successor(evidence):
    return frozen_gate.frozen_verdict(evidence, successor=True)


def test_decision82_gate_matrix():
    evidence = rust_measurement()
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        sample["seconds"] = 5
    assert successor(evidence)["outcome"] == "no-go"
    evidence["size"] = "large"
    assert successor(evidence)["outcome"] == "pass"
    assert successor(evidence)["speed"]["fix_items"]
    evidence["repetitions"][0]["cells"]["rust"]["samples"].extend(
        {"route_id": "R27", "seconds": 0.1, "status": 503, "rps": 10.0}
        for _ in range(2000)
    )
    assert successor(evidence)["outcome"] == "no-go"
    # The historical first-cutover gate is deliberately unchanged.
    assert frozen_gate.frozen_verdict(evidence)["outcome"] == "pass"


def test_lower_rust_rate_is_valid_with_frozen_baseline():
    evidence = rust_measurement()
    evidence["rates_rps"] = [2.0]
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        sample["rps"] = 2.0
    assert successor(evidence)["outcome"] == "pass"


@pytest.mark.parametrize("defect", ["missing-route", "floor", "nan", "transport", "pin"])
def test_invalid_evidence_never_passes(defect):
    evidence = rust_measurement()
    samples = evidence["repetitions"][0]["cells"]["rust"]["samples"]
    if defect == "missing-route":
        samples[:] = [sample for sample in samples if sample["route_id"] != "R27"]
    elif defect == "floor":
        samples.pop()
    elif defect == "nan":
        samples[0]["seconds"] = float("nan")
    elif defect == "transport":
        samples.append({"route_id": "R27", "seconds": 0.1, "status": 0, "rps": 10.0})
    else:
        evidence["baseline_sha256"] = "0" * 64
    assert successor(evidence)["outcome"] != "pass"


def test_baseline_pin_cannot_be_overridden_with_mixed_source(tmp_path):
    document = copy.deepcopy(baseline.load_python_baseline())
    document["source_sha"] = "a" * 40
    path = tmp_path / "baseline.json"
    raw = json.dumps(document).encode()
    path.write_bytes(raw)
    with pytest.raises(ValueError):
        baseline.load_python_baseline(path, hashlib.sha256(raw).hexdigest(), successor=True)


def test_successor_bundle_requires_artifacts_not_booleans(tmp_path):
    from scripts.rust_contract.report import build_meaning_bundle

    evidence = rust_measurement()
    assert build_meaning_bundle(evidence, {"integration_passed": True, "differences": 0})["outcome"] == "inconclusive"
    # A known measured failure wins over missing correctness evidence.
    for sample in evidence["repetitions"][0]["cells"]["rust"]["samples"]:
        sample["seconds"] = 5
    assert build_meaning_bundle(evidence, {})["outcome"] == "no-go"

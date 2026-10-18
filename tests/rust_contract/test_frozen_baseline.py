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

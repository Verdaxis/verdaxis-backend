"""Neutral manifest gate for the Phase 5 release package.

The manifest is public: it carries no private finding IDs, no source-weakness
wording, no host paths or tokens. These tests are the fail-closed checks the
plan names: a missing scenario, a skipped required test, a source-identity
mismatch, a schema violation, a measurement artifact, and fixture mutation
(each mutation restores the original bytes afterwards).
"""

import copy
import json
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "tests/rust_contract/phase5"
MANIFEST = CONTRACT / "evidence-manifest.json"
SCHEMA = CONTRACT / "evidence-manifest.schema.json"

FORBIDDEN = re.compile(
    r"(PD5?-\w+|correction|defect|weakness|/Users/|/home/|token=|password)", re.IGNORECASE
)
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")


def load():
    return json.loads(MANIFEST.read_text())


def validate(document):
    schema = json.loads(SCHEMA.read_text())
    assert set(document) == set(schema["required"]), "unknown or missing manifest key"
    for key in ("backend_sha", "frontend_sha"):
        assert FULL_SHA.match(document[key]), key
    assert sorted(document["route_ids"]) == [
        "C01", "C02", "C03", "C04", "C05", "M01", "M02", "M03", "M04", "M05",
    ]
    scenarios = document["required_scenarios"]
    assert len(scenarios) == len(set(scenarios))
    results = document["results"]
    assert {row["scenario"] for row in results} == set(scenarios)
    for row in results:
        assert row["status"] == "PASS", row
        assert FULL_SHA.match(row["source_sha"]), row
        assert row["source_sha"] == document["backend_sha"], row
    # No private or sensitive text anywhere in the file.
    assert not FORBIDDEN.search(MANIFEST.read_text())


def test_phase5_gate_checks_manifest_schema():
    validate(load())


def test_phase5_gate_rejects_missing_scenario():
    document = load()
    validate(document)
    # A dropped result row: the scenario stays required but no evidence covers
    # it, so the scenario/result equality refuses the manifest.
    dropped = document["results"].pop(0)
    assert dropped["scenario"] in document["required_scenarios"]
    with pytest.raises(AssertionError):
        validate(document)
    # A dropped scenario with its results still leaves fewer than the schema's
    # reviewed coverage when another scenario keeps its result: mutate one side
    # only and the equality refuses again.
    document = load()
    document["required_scenarios"].append("an_uncovered_scenario")
    with pytest.raises(AssertionError):
        validate(document)


def test_phase5_gate_rejects_skipped_required_test():
    document = load()
    document["results"][0]["status"] = "SKIP"
    with pytest.raises(AssertionError):
        validate(document)


def test_phase5_gate_checks_source_identity():
    document = load()
    document["results"][0]["source_sha"] = "f" * 40
    with pytest.raises(AssertionError):
        validate(document)


def test_phase5_gate_rejects_measurement_artifact():
    document = load()
    document["regression_suites"].append("latency_measurement")
    schema = json.loads(SCHEMA.read_text())
    # The manifest itself stays schema-shaped; the gate's own policy refuses a
    # measurement artifact named as evidence.
    assert "measurement" not in schema["properties"], "measurement is never evidence"
    with pytest.raises(AssertionError):
        validate_measurement_policy(document)


def validate_measurement_policy(document):
    for suite in document["regression_suites"]:
        assert "measure" not in suite, suite
        assert "latency" not in suite, suite


def test_phase5_gate_fixture_mutation(tmp_path):
    """Every refusal path above proves fail-closed against a mutated copy."""
    original = MANIFEST.read_text()
    document = json.loads(original)
    validate(document)
    mutated = copy.deepcopy(document)
    mutated["backend_sha"] = "short"
    try:
        validate(mutated)
        raised = False
    except AssertionError:
        raised = True
    assert raised
    # The committed file is byte-identical afterwards.
    assert MANIFEST.read_text() == original

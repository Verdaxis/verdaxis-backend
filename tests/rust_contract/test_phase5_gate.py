"""Neutral manifest gate for the Phase 5 release package.

The manifest is public: it carries no private finding IDs, no source-weakness
wording, no host paths or tokens. `refusals` is the gate: it returns every
reason to refuse a manifest, and the committed manifest must return none.
The tests below prove the gate fails closed: a missing scenario, a skipped
required test, a source-identity mismatch, a schema violation, a measurement
artifact and a mutated fixture file are each refused, and restoring the file
passes again.
"""

import copy
import json
import re
import subprocess
from pathlib import Path

import jsonschema
import pytest

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "tests/rust_contract/phase5"
MANIFEST = CONTRACT / "evidence-manifest.json"
SCHEMA = CONTRACT / "evidence-manifest.schema.json"
RUST_TESTS = ROOT / "rs/read/tests"

# The scenarios every release must cover, each with the one test that proves it.
REQUIRED = {
    "public_boundary": "phase5_public_boundary",
    "authenticated_boundary": "phase5_authenticated_boundary",
    "tenant_visibility": "phase5_visibility_cases",
    "snapshot_no_business_write": "phase5_snapshot_cases",
    "cancellation_ownership": "phase5_read_limits",
    "result_bounds": "run_phase5_bounds",
    "routing_matrix": "phase5_routing_cases",
    "cache_ledger_exclusions": "test_public_read_cache_events",
}
# The files that make up the release package. They may change after the source
# commit the manifest names, because the manifest is recorded last.
RELEASE_PACKAGE = {
    "tests/rust_contract/phase5/evidence-manifest.json",
    "tests/rust_contract/phase5/evidence-manifest.schema.json",
    "tests/rust_contract/test_phase5_gate.py",
    ".github/workflows/rust-read-ci.yml",
    "ARCHITECTURE.md",
}
ROUTE_IDS = ["C01", "C02", "C03", "C04", "C05", "M01", "M02", "M03", "M04", "M05"]
FORBIDDEN = re.compile(
    r"(PD5?-\w+|correction|defect|weakness|/Users/|/home/|token=|password)", re.IGNORECASE
)
MEASUREMENT = re.compile(r"(measure|latency|throughput|capacity|speed)", re.IGNORECASE)
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")


def load(path=MANIFEST):
    return json.loads(path.read_text())


def identifier_exists(identifier):
    """Whether a named test target, test function, Python test or runner exists."""
    for suffix in (".rs", ".sh"):
        if (RUST_TESTS / f"{identifier}{suffix}").is_file():
            return True
    function = re.compile(rf"\bfn {re.escape(identifier)}\(")
    if any(function.search(path.read_text()) for path in RUST_TESTS.glob("*.rs")):
        return True
    return any((ROOT / "tests").rglob(f"{identifier}.py"))


def refusals(document):
    """Every reason the manifest is not release-eligible evidence; empty if none."""
    found = [
        f"schema: {'/'.join(map(str, error.path))}: {error.message}"
        for error in jsonschema.Draft7Validator(json.loads(SCHEMA.read_text())).iter_errors(document)
    ]
    if found:
        return found
    if sorted(document["route_ids"]) != ROUTE_IDS:
        found.append("route ids differ from the ten Phase 5 routes")
    if sorted(document["required_scenarios"]) != sorted(REQUIRED):
        found.append("required scenarios differ from the release scenarios")
    results = {row["scenario"]: row for row in document["results"]}
    if len(results) != len(document["results"]):
        found.append("a scenario has more than one result")
    for scenario, identifier in REQUIRED.items():
        row = results.get(scenario)
        if row is None:
            found.append(f"missing result: {scenario}")
            continue
        if row["status"] != "PASS":
            found.append(f"required test did not pass: {scenario}")
        if row["source_sha"] != document["backend_sha"]:
            found.append(f"result is for another source: {scenario}")
        if row["test_identifier"] != identifier or not identifier_exists(identifier):
            found.append(f"result names no existing required test: {scenario}")
    for suite in document["regression_suites"]:
        if MEASUREMENT.search(suite):
            found.append(f"a measurement is not release evidence: {suite}")
    if FORBIDDEN.search(json.dumps(document)):
        found.append("private or sensitive text")
    return found


def test_phase5_gate_checks_manifest_schema():
    document = load()
    assert refusals(document) == []
    extra = copy.deepcopy(document)
    extra["note"] = "unknown key"
    assert any(reason.startswith("schema") for reason in refusals(extra))
    missing = copy.deepcopy(document)
    del missing["bounds_version"]
    assert any(reason.startswith("schema") for reason in refusals(missing))
    for key in ("backend_sha", "frontend_sha"):
        short = copy.deepcopy(document)
        short[key] = document[key][:7]
        assert any(reason.startswith("schema") for reason in refusals(short)), key
    unknown_route = copy.deepcopy(document)
    unknown_route["route_ids"][0] = "X01"
    assert refusals(unknown_route)
    result_key = copy.deepcopy(document)
    result_key["results"][0]["notes"] = "unknown key"
    assert any(reason.startswith("schema") for reason in refusals(result_key))


def test_phase5_gate_rejects_missing_scenario():
    document = load()
    # A dropped result leaves the scenario required but unproven.
    dropped = copy.deepcopy(document)
    scenario = dropped["results"].pop(0)["scenario"]
    assert f"missing result: {scenario}" in refusals(dropped)
    # Dropping the scenario and its result together is refused too: the gate
    # owns the list of release scenarios, not the manifest.
    both = copy.deepcopy(document)
    both["required_scenarios"].remove(scenario)
    both["results"] = [row for row in both["results"] if row["scenario"] != scenario]
    assert "required scenarios differ from the release scenarios" in refusals(both)
    # An extra, unknown scenario is not accepted either.
    extra = copy.deepcopy(document)
    extra["required_scenarios"].append("an_uncovered_scenario")
    assert refusals(extra)
    duplicate = copy.deepcopy(document)
    duplicate["results"].append(copy.deepcopy(duplicate["results"][0]))
    assert "a scenario has more than one result" in refusals(duplicate)


def test_phase5_gate_rejects_skipped_required_test():
    document = load()
    for status in ("SKIP", "FAIL"):
        mutated = copy.deepcopy(document)
        mutated["results"][0]["status"] = status
        reasons = refusals(mutated)
        assert reasons and (
            any(reason.startswith("schema") for reason in reasons)
            or f"required test did not pass: {document['results'][0]['scenario']}" in reasons
        ), status
    # A result that names a test that does not exist proves nothing.
    renamed = copy.deepcopy(document)
    renamed["results"][0]["test_identifier"] = "no_such_required_test"
    assert any("names no existing required test" in reason for reason in refusals(renamed))


def test_phase5_gate_checks_source_identity():
    document = load()
    mismatched = copy.deepcopy(document)
    mismatched["results"][0]["source_sha"] = "f" * 40
    assert any("another source" in reason for reason in refusals(mismatched))
    # The named source is a commit of this history, and only the release
    # package (manifest, schema, this gate, its CI step and the architecture
    # note) differs between that commit and the commit that records it.
    source = document["backend_sha"]
    assert FULL_SHA.match(source)
    exists = subprocess.run(["git", "cat-file", "-e", f"{source}^{{commit}}"], cwd=ROOT,
                            capture_output=True, text=True)
    assert exists.returncode == 0, "the manifest source commit is not in this history"
    recorded = subprocess.run(
        ["git", "log", "-1", "--format=%H", "--", str(MANIFEST.relative_to(ROOT))],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    if not recorded:
        pytest.skip("the manifest is not committed yet")
    ancestor = subprocess.run(["git", "merge-base", "--is-ancestor", source, recorded], cwd=ROOT)
    assert ancestor.returncode == 0, "the manifest source is not an ancestor of the recording commit"
    changed = subprocess.run(["git", "diff", "--name-only", source, recorded], cwd=ROOT,
                             capture_output=True, text=True, check=True).stdout.split()
    assert set(changed) <= RELEASE_PACKAGE, changed


def test_phase5_gate_rejects_measurement_artifact():
    document = load()
    for suite in ("latency_measurement", "memory_capacity_run", "rust_contract_measure"):
        mutated = copy.deepcopy(document)
        mutated["regression_suites"].append(suite)
        assert any("not release evidence" in reason for reason in refusals(mutated)), suite
    assert not any(MEASUREMENT.search(suite) for suite in document["regression_suites"])


def test_phase5_gate_rejects_private_text():
    document = load()
    for text in ("PD5-X1 note", "a correction", "/home/runner/x", "token=abc"):
        mutated = copy.deepcopy(document)
        mutated["schema_revision"] = text
        assert "private or sensitive text" in refusals(mutated), text


def test_phase5_gate_fixture_mutation(tmp_path):
    """Mutate a copy of the manifest file, prove the refusal, restore, prove the pass."""
    copy_path = tmp_path / "evidence-manifest.json"
    original = MANIFEST.read_text()
    copy_path.write_text(original)
    assert refusals(load(copy_path)) == []

    # Remove one required scenario and its result.
    document = json.loads(original)
    scenario = document["required_scenarios"].pop()
    document["results"] = [row for row in document["results"] if row["scenario"] != scenario]
    copy_path.write_text(json.dumps(document))
    assert refusals(load(copy_path))
    copy_path.write_text(original)
    assert refusals(load(copy_path)) == []

    # Change one PASS to SKIP.
    document = json.loads(original)
    document["results"][0]["status"] = "SKIP"
    copy_path.write_text(json.dumps(document))
    assert refusals(load(copy_path))
    copy_path.write_text(original)
    assert refusals(load(copy_path)) == []

    # The committed file was never touched.
    assert MANIFEST.read_text() == original

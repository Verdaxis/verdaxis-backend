"""Required evidence cannot be reduced by deleting a file, case or runtime."""

import hashlib
import importlib
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tests/rust_contract/phase2/evidence-manifest.json"
SHA = "a" * 40
DIGEST = "b" * 64


def module():
    assert (ROOT / "scripts/rust_contract/phase2_report.py").is_file(), "missing Phase 2 evidence gate"
    return importlib.import_module("scripts.rust_contract.phase2_report")


def put(directory, name, value):
    raw = json.dumps(value).encode()
    (directory / name).write_bytes(raw)
    return {"path": name, "sha256": hashlib.sha256(raw).hexdigest()}


def fixture(tmp_path, *, production=False):
    manifest = json.loads(MANIFEST.read_text())
    identity = {key: DIGEST for key in manifest["required_identity_fields"]}
    identity.update(candidate_sha=SHA, oracle_sha=manifest["oracle_sha"], schema_revision="test-revision",
                    session_time_zone="UTC", python_database_identity="python-db", rust_database_identity="rust-db")
    identity["normalization_sha256"] = hashlib.sha256((MANIFEST.parent / "normalization.json").read_bytes()).hexdigest()
    identity["comparator_profile_sha256"] = hashlib.sha256((ROOT / "scripts/rust_contract/phase2_meaning.py").read_bytes()).hexdigest()
    inputs = {}
    kinds = manifest["scopes"]["production-cutover" if production else "disposable-artifact"]
    scenarios = [row["id"] for row in manifest["required_scenarios"]]
    for kind in kinds:
        run = {"kind": kind, "identity": identity, "id": kind, "start": "2026-10-18T00:00:00Z", "end": "2026-10-18T00:01:00Z",
               "exit_code": 0, "expected_cases": scenarios, "observed_cases": scenarios, "skips": 0, "failures": 0,
               "input_sha256": DIGEST, "cells": []}
        for runtime in ("python", "rust"):
            for size in ("small", "large"):
                for kyc in (False, True):
                    run["cells"].append({"runtime": runtime, "size": size, "kyc": kyc,
                                         "cases": [{"id": case, "differences": [], "python": {"status": 200, "headers": {"content-type": "application/json"}, "body": "{}"}, "rust": {"status": 200, "headers": {"content-type": "application/json"}, "body": "{}"}} for case in scenarios]})
        if kind in {"committed-state", "side-effects", "concurrency", "cleanup", "order-write-benchmark", "release-rehearsal"}:
            for cell in run["cells"]:
                for case in cell["cases"]:
                    case["proof"] = {"python_before": {}, "rust_before": {}, "python_after": {}, "rust_after": {}, "python_effects": [], "rust_effects": [], "transaction": "committed", "cleanup_remaining": []}
        if kind == "integration-junit":
            xml = b'<testsuites><testsuite name="required"><testcase name="integration"/></testsuite></testsuites>'
            (tmp_path / "integration.xml").write_bytes(xml)
            run["junit"] = {"path": "integration.xml", "sha256": hashlib.sha256(xml).hexdigest()}
        if kind == "artifact-provenance":
            from cryptography.hazmat.primitives import serialization
            from cryptography.hazmat.primitives.asymmetric.ed25519 import (
                Ed25519PrivateKey,
            )
            key = Ed25519PrivateKey.from_private_bytes(b"x" * 32)
            (tmp_path / "trusted.pem").write_bytes(key.public_key().public_bytes(serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo))
            record = {"version": 1, "repository": "Verdaxis/verdaxis-backend", "workflow_path": ".github/workflows/rust-read-ci.yml", "workflow_ref": "refs/heads/feat/rust-phase-2", "source_sha": SHA, "target": "x86_64-unknown-linux-gnu", "archive_sha256": DIGEST, "binary_sha256": DIGEST, "attested_subjects": ["verdaxis-read", "archive"], "gh_attestation_verified": True, "attested_at": "2026-10-18T00:00:00Z"}
            record_ref = put(tmp_path, "provenance.json", record)
            signature = key.sign((tmp_path / "provenance.json").read_bytes())
            (tmp_path / "signature").write_bytes(signature)
            run["artifact"] = {"record": record_ref, "signature": {"path": "signature", "sha256": hashlib.sha256(signature).hexdigest()}}
        if kind == "measurement":
            run["floor"] = 1000
            run["weights"] = {route: 1 for route in manifest["route_ids"]}
            run["measurements"] = [
                {"route_id": route, "runtime": runtime, "size": size, "success": 1000,
                 "p95": 0.2, "statuses": {"200": 1000}, "transport_failures": 0,
                 "samples": [0.2] * 1000}
                for route in manifest["route_ids"] for runtime in ("python", "rust") for size in ("small", "large")]
            run["workload"] = DIGEST
            run["repetitions"] = 1
            run["runtime_topology"] = {"python_workers": 4, "rust_processes": 1}
            run["memory"] = {"python": 100, "rust": 90}
        inputs[kind] = put(tmp_path, kind + ".json", run)
    index = {"identity": identity, "scope": "production-cutover" if production else "disposable-artifact", "inputs": inputs}
    (tmp_path / "index.json").write_text(json.dumps(index))
    return index


def evaluate(tmp_path, scope="disposable-artifact"):
    return module().evaluate(MANIFEST, scope, tmp_path, candidate_sha=SHA, binary_sha256=DIGEST, provenance_key=tmp_path / "trusted.pem")


def change(tmp_path, kind, mutate):
    index = json.loads((tmp_path / "index.json").read_text())
    path = tmp_path / index["inputs"][kind]["path"]
    run = json.loads(path.read_text())
    mutate(run)
    index["inputs"][kind] = put(tmp_path, path.name, run)
    (tmp_path / "index.json").write_text(json.dumps(index))


@pytest.mark.parametrize("content", [None, {}, {"inputs": {}}, {"inputs": {"responses": {}}}])
def test_missing_evidence_is_inconclusive(tmp_path, content):
    if content is not None:
        (tmp_path / "index.json").write_text(json.dumps(content))
    assert evaluate(tmp_path)["verdict"] == "INCONCLUSIVE"


def test_disposable_scope_cannot_authorize_cutover(tmp_path):
    fixture(tmp_path)
    assert evaluate(tmp_path)["verdict"] == "GO"
    assert evaluate(tmp_path, "production-cutover")["verdict"] == "INCONCLUSIVE"


@pytest.mark.parametrize("mutation", [
    lambda run: run.update(cells=[]),
    lambda run: run["cells"].pop(),
    lambda run: run["cells"][0]["cases"].pop(),
    lambda run: run["cells"][0]["cases"].append(run["cells"][0]["cases"][0]),
    lambda run: run.update(skips=True),
    lambda run: run.update(failures=-1),
    lambda run: run.update(exit_code=1),
    lambda run: run["identity"].update(candidate_sha="c" * 40),
    lambda run: run.update(observed_cases=[]),
])
def test_partial_invalid_or_stale_response_evidence_is_inconclusive(tmp_path, mutation):
    fixture(tmp_path)
    change(tmp_path, "responses", mutation)
    assert evaluate(tmp_path)["verdict"] == "INCONCLUSIVE"


def test_hash_mismatch_and_path_escape_are_inconclusive(tmp_path):
    index = fixture(tmp_path)
    (tmp_path / "responses.json").write_text("{}")
    assert evaluate(tmp_path)["verdict"] == "INCONCLUSIVE"
    index["inputs"]["responses"]["path"] = "../outside.json"
    (tmp_path / "index.json").write_text(json.dumps(index))
    assert evaluate(tmp_path)["verdict"] == "INCONCLUSIVE"


@pytest.mark.parametrize("xml", [b'<testsuites/>', b'<testsuite tests="1"/>', b'<testsuite><testcase><skipped/></testcase></testsuite>', b'bad'])
def test_invalid_zero_or_skipped_junit_is_inconclusive(tmp_path, xml):
    fixture(tmp_path)
    (tmp_path / "integration.xml").write_bytes(xml)
    change(tmp_path, "integration-junit", lambda run: run["junit"].update(sha256=hashlib.sha256(xml).hexdigest()))
    assert evaluate(tmp_path)["verdict"] == "INCONCLUSIVE"


def test_valid_failure_wins_over_missing_evidence(tmp_path):
    fixture(tmp_path)
    def mark_failure(run):
        run["cells"][0]["cases"][0]["differences"] = ["/body:keys"]
        run["cells"][0]["cases"][0]["rust"]["body"] = '{"extra":true}'
        run["failures"] = 1
    change(tmp_path, "responses", mark_failure)
    (tmp_path / "cleanup.json").unlink()
    result = evaluate(tmp_path)
    assert result["verdict"] == "NO_GO"
    assert result["incomplete"]


@pytest.mark.parametrize("mutation", [
    lambda run: run["measurements"].pop(),
    lambda run: run["measurements"][0].update(success=999),
    lambda run: run["measurements"][0].update(success=True),
    lambda run: run["measurements"][0].update(p95=float("nan")),
    lambda run: run["measurements"][0].update(p95=-1),
    lambda run: run["measurements"][0].update(samples=[]),
    lambda run: run.update(floor=999),
    lambda run: run["weights"].pop("N01"),
])
def test_measurement_requires_each_runtime_route_size_and_floor(tmp_path, mutation):
    fixture(tmp_path, production=True)
    change(tmp_path, "measurement", mutation)
    assert evaluate(tmp_path, "production-cutover")["verdict"] == "INCONCLUSIVE"


@pytest.mark.parametrize("python,rust,blocks", [(0.4, 0.5, False), (0.4, 0.500001, True), (0.1, 0.15, False), (0.1, 0.150001, True)])
def test_route_caps_reuse_shared_gate(python, rust, blocks):
    assert module().route_regression_blocks(python, rust) is blocks


def test_worse_weighted_mix_is_no_go(tmp_path):
    fixture(tmp_path, production=True)
    def mutate(run):
        for row in run["measurements"]:
            if row["runtime"] == "rust":
                row.update(p95=0.21, samples=[0.21] * 1000)
    change(tmp_path, "measurement", mutate)
    assert evaluate(tmp_path, "production-cutover")["verdict"] == "NO_GO"


def test_renderer_uses_json_verdict_and_cli_exit_codes(tmp_path):
    fixture(tmp_path)
    out_json, out_md = tmp_path / "gate.json", tmp_path / "gate.md"
    args = ["--manifest", str(MANIFEST), "--scope", "disposable-artifact", "--evidence-dir", str(tmp_path),
            "--candidate-sha", SHA, "--binary-sha256", DIGEST, "--provenance-key", str(tmp_path / "trusted.pem"), "--out-json", str(out_json), "--out-markdown", str(out_md)]
    assert module().main(args) == 0
    verdict = json.loads(out_json.read_text())
    assert "GO" in out_md.read_text()
    verdict["verdict"] = "NO_GO"
    assert "NO_GO" in module().render_markdown(verdict)
    assert module().main([a if a != "disposable-artifact" else "production-cutover" for a in args]) == 2

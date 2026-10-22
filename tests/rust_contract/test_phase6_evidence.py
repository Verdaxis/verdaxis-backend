"""Synthetic, source-bound records for the stream acceptance gate."""
from __future__ import annotations

import copy
import hashlib
import importlib
import json
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
INVENTORY = json.loads((ROOT / "tests/rust_contract/phase6/required-scenarios.json").read_text())


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def commit(repo, subject):
    git(repo, "add", "cases.txt", "implementation.txt")
    git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
        "-c", "commit.gpgsign=false", "commit", "-qm", subject)
    return git(repo, "rev-parse", "HEAD")


@pytest.fixture
def evidence(tmp_path):
    repo = tmp_path / "source"
    repo.mkdir()
    git(repo, "init", "-q")
    (repo / "cases.txt").write_text("synthetic cases\n")
    (repo / "implementation.txt").write_text("initial\n")
    red_sha = commit(repo, "test: define cases")
    (repo / "implementation.txt").write_text("complete\n")
    green_sha = commit(repo, "feat: implement cases")
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    scenarios = INVENTORY["scenarios"]
    bundle = {
        "version": 1, "scope": "cutover-rehearsal", "source_sha": green_sha,
        "release_sha": green_sha, "fixture_sha256": digest(b"fixture"),
        "implementer": "builder", "runs": [], "tasks": [],
        "coexistence": {"python-leader": "green", "rust-leader": "green"},
        "review": {"reviewer": "reviewer", "source_sha": green_sha,
                   "status": "approved", "run": "green"},
        "drain": {"run": "green", "rust_closed": True, "python_closed": True,
                  "rollback_reconnected": True},
        "publication": {"requested": False, "approved_shas": []},
    }
    cases = {case["id"]: case for case in scenarios}
    test_digest = digest((repo / "cases.txt").read_bytes())

    def add_run(run_id, sha, ids, exit_code, minute):
        suite = ET.Element("testsuite")
        for case_id in ids:
            case = cases[case_id]
            node = ET.SubElement(suite, "testcase", name=case["test"], classname=case_id)
            if exit_code:
                ET.SubElement(node, "failure", message="synthetic assertion")
        xml = ET.tostring(suite)
        (artifacts / f"{run_id}.xml").write_bytes(xml)
        start = f"2026-10-22T10:{minute:02d}:00Z"
        end = f"2026-10-22T10:{minute:02d}:01Z"
        run = {
            "id": run_id, "source_sha": sha, "tree": git(repo, "rev-parse", f"{sha}^{{tree}}"),
            "host": "staging", "command": "fixture-command", "started_at": start,
            "finished_at": end, "exit": exit_code, "cleanup_exit": 0,
            "features_enabled": True, "database_exercised": True,
            "fixture_sha256": bundle["fixture_sha256"], "executed": len(ids),
            "skipped": 0, "junit": {"path": f"{run_id}.xml", "sha256": digest(xml)},
            "cases": [{"id": case_id, "test": cases[case_id]["test"],
                       "test_path": "cases.txt", "test_sha256": test_digest,
                       "status": "failed" if exit_code else "passed"} for case_id in ids],
        }
        events = [
            {"event": "start", "run": run_id, "source_sha": sha, "tree": run["tree"],
             "host": run["host"], "command": run["command"], "at": start},
            {"event": "finish", "run": run_id, "at": end, "exit": exit_code,
             "cleanup_exit": 0, "executed": len(ids), "skipped": 0,
             "junit_sha256": digest(xml)},
        ]
        log = ("\n".join(json.dumps(event) for event in events)
               + f"\nCALL_EXIT={exit_code}\n").encode()
        (artifacts / f"{run_id}.log").write_bytes(log)
        run["log"] = {"path": f"{run_id}.log", "sha256": digest(log)}
        bundle["runs"].append(run)
        return run

    task_ids = sorted({case["task"] for case in scenarios} - {"13"})
    for index, task in enumerate(task_ids):
        ids = [case["id"] for case in scenarios if case["task"] == task]
        add_run(f"red-{task}", red_sha, ids, 1, index)
        bundle["tasks"].append({"task": task, "implementation_sha": green_sha,
                                "red": f"red-{task}", "green": "green"})
    add_run("green", green_sha, list(cases), 0, 40)
    return bundle, artifacts, repo


def validate(evidence):
    bundle, artifacts, repo = evidence
    module_path = ROOT / "scripts/rust_contract/phase6_evidence.py"
    assert module_path.is_file(), "stream evidence gate is not implemented"
    gate = importlib.import_module("scripts.rust_contract.phase6_evidence")
    return gate.validate_evidence(bundle, artifact_root=artifacts, repository=repo)


def refused(evidence):
    with pytest.raises(ValueError):
        validate(evidence)


def green(evidence):
    return evidence[0]["runs"][-1]


def test_evidence_complete_bundle_passes(evidence):
    verdict = validate(evidence)
    assert verdict == {"scope": "cutover-rehearsal", "source_sha": evidence[0]["source_sha"],
                       "scenarios": len(INVENTORY["scenarios"]), "publication": "not-requested"}


def test_evidence_missing_case_refused(evidence):
    green(evidence)["cases"] = green(evidence)["cases"][1:]
    refused(evidence)


def test_evidence_skipped_database_refused(evidence):
    green(evidence)["database_exercised"] = False
    refused(evidence)


def test_evidence_missing_leader_direction_refused(evidence):
    del evidence[0]["coexistence"]["rust-leader"]
    refused(evidence)


def test_evidence_missing_journey_refused(evidence):
    green(evidence)["cases"] = [c for c in green(evidence)["cases"] if c["id"] != "J61"]
    refused(evidence)


def test_evidence_missing_review_refused(evidence):
    del evidence[0]["review"]
    refused(evidence)


def test_evidence_missing_drain_refused(evidence):
    del evidence[0]["drain"]
    refused(evidence)


def test_evidence_missing_exit_refused(evidence):
    del green(evidence)["exit"]
    refused(evidence)


def test_evidence_identity_mismatch_refused(evidence):
    evidence[0]["release_sha"] = "0" * 40
    refused(evidence)


def test_evidence_missing_red_refused(evidence):
    del evidence[0]["tasks"][0]["red"]
    refused(evidence)


def test_evidence_red_after_implementation_refused(evidence):
    evidence[0]["tasks"][0]["red"] = "green"
    refused(evidence)


def test_evidence_red_tree_mismatch_refused(evidence):
    evidence[0]["runs"][0]["tree"] = "0" * 40
    refused(evidence)


def test_evidence_unreleased_push_refused(evidence):
    evidence[0]["publication"]["requested"] = True
    refused(evidence)


@pytest.mark.parametrize("field,value", [
    ("exit", 1), ("cleanup_exit", 1), ("executed", 0), ("skipped", 1),
    ("features_enabled", False), ("source_sha", "0" * 40),
    ("fixture_sha256", "0" * 64), ("finished_at", "2026-10-22T09:00:00Z"),
])
def test_evidence_run_completion_required(evidence, field, value):
    green(evidence)[field] = value
    refused(evidence)


@pytest.mark.parametrize("artifact", ["log", "junit"])
def test_evidence_artifact_digest_required(evidence, artifact):
    green(evidence)[artifact]["sha256"] = "0" * 64
    refused(evidence)


def test_evidence_log_exit_marker_required(evidence):
    run = green(evidence)
    path = evidence[1] / run["log"]["path"]
    log = path.read_bytes().replace(b"CALL_EXIT=0\n", b"")
    path.write_bytes(log)
    run["log"]["sha256"] = digest(log)
    refused(evidence)


def test_evidence_duplicate_run_refused(evidence):
    evidence[0]["runs"].append(copy.deepcopy(green(evidence)))
    refused(evidence)


def test_evidence_test_digest_required(evidence):
    evidence[0]["runs"][0]["cases"][0]["test_sha256"] = "0" * 64
    refused(evidence)


def test_evidence_self_review_refused(evidence):
    evidence[0]["review"]["reviewer"] = evidence[0]["implementer"]
    refused(evidence)


def test_evidence_artifact_escape_refused(evidence):
    green(evidence)["log"]["path"] = "../outside.log"
    refused(evidence)


def test_evidence_publication_exact_ancestry_required(evidence):
    bundle, _, repo = evidence
    publication = bundle["publication"]
    publication.update(requested=True, base_sha=bundle["runs"][0]["source_sha"],
                       approved_shas=[bundle["source_sha"]], approver="PM",
                       reviewed_shas=[bundle["source_sha"]])
    assert git(repo, "rev-list", f'{publication["base_sha"]}..{bundle["source_sha"]}')
    assert validate(evidence)["publication"] == "approved"
    publication["approved_shas"] = []
    refused(evidence)


def test_evidence_candidate_does_not_require_cutover_cases(evidence):
    evidence[0]["scope"] = "candidate"
    del evidence[0]["drain"]
    assert validate(evidence)["scope"] == "candidate"


def test_evidence_red_order_requires_host_log(evidence):
    run = evidence[0]["runs"][0]
    path = evidence[1] / run["log"]["path"]
    lines = path.read_text().splitlines()
    lines[0], lines[1] = lines[1], lines[0]
    log = ("\n".join(lines) + "\n").encode()
    path.write_bytes(log)
    run["log"]["sha256"] = digest(log)
    refused(evidence)


def test_evidence_missing_task_refused(evidence):
    evidence[0]["tasks"].pop()
    refused(evidence)

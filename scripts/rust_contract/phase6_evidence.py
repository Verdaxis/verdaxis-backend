"""Validate neutral stream acceptance records against local source and artifacts.

Records are supplied by a trusted host recorder. Hashes and Git ancestry check
consistency, not recorder identity or permission to deploy. The gate never runs
commands from evidence, opens a database, or prints captured test output.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INVENTORY = ROOT / "tests/rust_contract/phase6/required-scenarios.json"
SCOPES = {"candidate", "cutover-rehearsal"}
SHA = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
MAX_ARTIFACT_BYTES = 16 * 1024 * 1024


class EvidenceError(ValueError):
    """A bounded, neutral rejection reason, without private record contents."""


def require(condition, reason):
    if not condition:
        raise EvidenceError(reason)


def load_json(path):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "duplicate JSON field")
            result[key] = value
        return result

    try:
        return json.loads(Path(path).read_text(), object_pairs_hook=unique_object)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError("JSON record unavailable") from error


def timestamp(value):
    require(isinstance(value, str) and value.endswith("Z"), "UTC timestamp required")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise EvidenceError("invalid timestamp") from error
    require(parsed.utcoffset() == timezone.utc.utcoffset(parsed), "UTC timestamp required")
    return parsed


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def integer(value):
    return type(value) is int


class Source:
    def __init__(self, repository):
        self.repository = Path(repository)

    def git(self, *args):
        result = subprocess.run(
            ["git", "--no-replace-objects", "-C", str(self.repository), *args],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
        require(result.returncode == 0, "source verification failed")
        return result.stdout

    def commit(self, sha):
        require(isinstance(sha, str) and SHA.fullmatch(sha), "full source SHA required")
        require(self.git("cat-file", "-t", sha).strip() == b"commit", "commit required")

    def tree(self, sha):
        self.commit(sha)
        return self.git("rev-parse", f"{sha}^{{tree}}").decode().strip()

    def ancestor(self, before, after, *, strict=False):
        self.commit(before)
        self.commit(after)
        require(not strict or before != after, "distinct source commits required")
        self.git("merge-base", "--is-ancestor", before, after)

    def test_digest(self, sha, path):
        require(isinstance(path, str) and path and not Path(path).is_absolute()
                and ".." not in Path(path).parts and ":" not in path,
                "relative test source required")
        return sha256(self.git("show", f"{sha}:{path}"))


def artifact(reference, root):
    require(isinstance(reference, dict), "artifact reference required")
    name = reference.get("path")
    expected = reference.get("sha256")
    require(isinstance(name, str) and name and not Path(name).is_absolute(),
            "relative artifact path required")
    require(isinstance(expected, str) and DIGEST.fullmatch(expected), "artifact digest required")
    path = (root / name).resolve()
    require(path.is_relative_to(root), "artifact outside evidence root")
    try:
        require(path.stat().st_size <= MAX_ARTIFACT_BYTES, "artifact exceeds bound")
        data = path.read_bytes()
    except OSError as error:
        raise EvidenceError("artifact unavailable") from error
    require(sha256(data) == expected, "artifact digest mismatch")
    return data


def junit_results(data):
    require(b"<!DOCTYPE" not in data and b"<!ENTITY" not in data, "plain JUnit XML required")
    try:
        root = ET.fromstring(data)
    except ET.ParseError as error:
        raise EvidenceError("invalid JUnit artifact") from error
    require(root.tag in {"testsuite", "testsuites"}, "JUnit suite required")
    results = {}
    for node in root.iter("testcase"):
        key = (node.get("classname"), node.get("name"))
        require(key not in results, "duplicate JUnit case")
        require(node.find("skipped") is None, "skipped case refused")
        results[key] = "failed" if (node.find("failure") is not None
                                   or node.find("error") is not None) else "passed"
    require(results, "populated JUnit required")
    return results


def ordered_log(data, run):
    try:
        lines = data.decode("utf-8").splitlines()
        require(len(lines) >= 3 and lines[-1] == f'CALL_EXIT={run["exit"]}',
                "terminal exit marker required")
        events = [json.loads(line) for line in lines[:-1]]
    except (UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError("ordered host log required") from error
    require(len(events) == 2, "start and finish log records required")
    expected_start = {"event": "start", "run": run["id"],
                      "source_sha": run["source_sha"], "tree": run["tree"],
                      "host": run["host"], "command": run["command"],
                      "at": run["started_at"]}
    expected_finish = {"event": "finish", "run": run["id"], "at": run["finished_at"],
                       "exit": run["exit"], "cleanup_exit": run["cleanup_exit"],
                       "executed": run["executed"], "skipped": run["skipped"],
                       "junit_sha256": run["junit"]["sha256"]}
    require(events == [expected_start, expected_finish], "host log and run disagree")


def validate_run(run, *, source, artifacts, cases, fixture):
    require(isinstance(run, dict), "run record required")
    fields = {"id", "source_sha", "tree", "host", "command", "started_at", "finished_at",
              "exit", "cleanup_exit", "features_enabled", "database_exercised",
              "fixture_sha256", "executed", "skipped", "junit", "log", "cases"}
    require(fields <= run.keys(), "incomplete run")
    require(isinstance(run["id"], str) and run["id"], "run identity required")
    require(run["host"] in {"m1", "staging", "prod"}, "test host required")
    require(isinstance(run["command"], str) and run["command"].strip(), "command required")
    require(source.tree(run["source_sha"]) == run["tree"], "source tree mismatch")
    require(timestamp(run["started_at"]) < timestamp(run["finished_at"]), "run order invalid")
    require(integer(run["exit"]) and run["exit"] in {0, 1}, "test exit required")
    require(integer(run["cleanup_exit"]) and run["cleanup_exit"] == 0, "cleanup failed")
    require(run["features_enabled"] is True, "test features required")
    require(run["fixture_sha256"] == fixture, "fixture identity mismatch")
    require(integer(run["skipped"]) and run["skipped"] == 0, "skipped suite refused")
    require(isinstance(run["cases"], list) and run["cases"], "executed cases required")
    require(integer(run["executed"]) and run["executed"] == len(run["cases"]),
            "executed count mismatch")
    results = junit_results(artifact(run["junit"], artifacts))
    require(len(results) == run["executed"], "JUnit count mismatch")
    by_id = {}
    for result in run["cases"]:
        require(isinstance(result, dict), "case result required")
        case_id = result.get("id")
        require(case_id in cases and case_id not in by_id, "unknown or repeated case")
        case = cases[case_id]
        require(result.get("test") == case["test"], "test identity mismatch")
        require(result.get("status") in {"passed", "failed"}, "case status required")
        require(results.get((case_id, case["test"])) == result["status"],
                "JUnit result mismatch")
        require(source.test_digest(run["source_sha"], result.get("test_path"))
                == result.get("test_sha256"), "test source digest mismatch")
        by_id[case_id] = result
    failed = any(result["status"] == "failed" for result in by_id.values())
    require(failed == (run["exit"] == 1), "exit and assertions disagree")
    ordered_log(artifact(run["log"], artifacts), run)
    return by_id


def validate_publication(record, source, candidate):
    require(isinstance(record, dict) and type(record.get("requested")) is bool,
            "publication state required")
    if not record["requested"]:
        return "not-requested"
    base = record.get("base_sha")
    source.ancestor(base, candidate, strict=True)
    commits = set(source.git("rev-list", f"{base}..{candidate}").decode().splitlines())
    for field in ("approved_shas", "reviewed_shas"):
        values = record.get(field)
        require(isinstance(values, list) and len(values) == len(set(values))
                and set(values) == commits, "exact publication ancestry required")
    require(isinstance(record.get("approver"), str) and record["approver"].strip(),
            "publication approver required")
    return "approved"


def _validate(bundle, artifact_root, repository):
    require(isinstance(bundle, dict) and type(bundle.get("version")) is int
            and bundle["version"] == 1, "evidence version required")
    scope = bundle.get("scope")
    require(scope in SCOPES, "evidence scope required")
    inventory = load_json(INVENTORY)
    cases = {case["id"]: case for case in inventory["scenarios"]}
    require(len(cases) == len(inventory["scenarios"])
            and all(case["required"] is True for case in cases.values()),
            "required inventory invalid")
    required = {key: case for key, case in cases.items() if scope in case["scopes"]}
    source = Source(repository)
    candidate = bundle.get("source_sha")
    source.commit(candidate)
    require(candidate == bundle.get("release_sha"), "release identity mismatch")
    fixture = bundle.get("fixture_sha256")
    require(isinstance(fixture, str) and DIGEST.fullmatch(fixture), "fixture digest required")
    artifacts = Path(artifact_root).resolve()
    runs = {}
    results = {}
    require(isinstance(bundle.get("runs"), list) and bundle["runs"], "run records required")
    for run in bundle["runs"]:
        run_results = validate_run(run, source=source, artifacts=artifacts,
                                   cases=cases, fixture=fixture)
        require(run["id"] not in runs, "duplicate run identity")
        source.ancestor(run["source_sha"], candidate)
        runs[run["id"]] = run
        results[run["id"]] = run_results
    passed = set()
    for run_id, run in runs.items():
        if run["exit"] == 0 and run["source_sha"] == candidate:
            passed.update(results[run_id])
    require(required.keys() <= passed, "required scenario missing")

    # Every implementation task, including this gate, needs a prior failing
    # assertion on unchanged test bytes and a later passing descendant run.
    task_ids = {case["task"] for case in required.values()} - {"13"}
    tasks = bundle.get("tasks")
    require(isinstance(tasks, list), "task records required")
    by_task = {task["task"]: task for task in tasks}
    require(len(by_task) == len(tasks) and task_ids <= by_task.keys(), "task coverage incomplete")
    for task_id in task_ids:
        task = by_task[task_id]
        red, green = runs[task["red"]], runs[task["green"]]
        implementation = task["implementation_sha"]
        source.ancestor(red["source_sha"], implementation, strict=True)
        source.ancestor(implementation, green["source_sha"])
        require(green["source_sha"] == candidate, "GREEN must attest candidate")
        require(red["exit"] == 1 and green["exit"] == 0, "RED and GREEN exits required")
        require(timestamp(red["finished_at"]) < timestamp(green["started_at"]),
                "RED must precede GREEN")
        task_cases = {key for key, case in required.items() if case["task"] == task_id}
        failures = task_cases & {key for key, result in results[red["id"]].items()
                                 if result["status"] == "failed"}
        require(failures, "task RED assertion required")
        require(task_cases <= results[green["id"]].keys(), "task GREEN coverage incomplete")
        for case_id in failures:
            before = results[red["id"]][case_id]
            after = results[green["id"]][case_id]
            require(after["status"] == "passed" and before["test_path"] == after["test_path"]
                    and before["test_sha256"] == after["test_sha256"],
                    "RED and GREEN test source mismatch")

    # Real database execution must be explicit for store, election and race
    # scenarios. Offline compilation or an optional skipped driver is not proof.
    for case_id, case in required.items():
        if case["task"] in {"2", "5", "6", "7", "11", "14a", "14b"}:
            require(any(run["exit"] == 0 and run["source_sha"] == candidate
                        and case_id in results[run_id] and run["database_exercised"] is True
                        and run["host"] in {"staging", "prod"}
                        for run_id, run in runs.items()), "database execution required")
    directions = bundle.get("coexistence")
    require(isinstance(directions, dict), "coexistence directions required")
    for direction in ("python-leader", "rust-leader"):
        run_id = directions.get(direction)
        require(run_id in runs and runs[run_id]["exit"] == 0
                and runs[run_id]["source_sha"] == candidate
                and runs[run_id]["database_exercised"] is True, "leader direction missing")
        required_case = "E129" if direction == "python-leader" else "E130"
        require(required_case in results[run_id], "leader direction case missing")
    review = bundle.get("review")
    require(isinstance(review, dict) and review.get("status") == "approved"
            and review.get("source_sha") == candidate, "candidate review required")
    implementer = bundle.get("implementer")
    require(isinstance(implementer, str) and implementer.strip(), "implementer required")
    require(isinstance(review.get("reviewer"), str) and review["reviewer"].strip()
            and review["reviewer"] != implementer, "independent reviewer required")
    review_run = review.get("run")
    require(review_run in runs and runs[review_run]["exit"] == 0
            and runs[review_run]["source_sha"] == candidate
            and {"E164", "E165", "E166"} <= results[review_run].keys(),
            "review results required")
    if scope == "cutover-rehearsal":
        drain = bundle.get("drain")
        require(isinstance(drain, dict), "drain evidence required")
        require(all(drain.get(key) is True for key in
                    ("rust_closed", "python_closed", "rollback_reconnected")),
                "drain outcome incomplete")
        run_id = drain.get("run")
        require(run_id in runs and runs[run_id]["exit"] == 0
                and runs[run_id]["source_sha"] == candidate
                and all(case_id in results[run_id] for case_id, case in required.items()
                        if case["task"] == "14b"), "drain scenarios incomplete")
    publication = validate_publication(bundle.get("publication"), source, candidate)
    return {"scope": scope, "source_sha": candidate, "scenarios": len(required),
            "publication": publication}


def validate_evidence(bundle, *, artifact_root, repository):
    """Return a neutral verdict or reject incomplete/inconsistent evidence."""
    try:
        return _validate(bundle, artifact_root, repository)
    except (KeyError, TypeError, AttributeError, OSError, UnicodeError) as error:
        raise EvidenceError("malformed evidence record") from error


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--repository", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    try:
        verdict = validate_evidence(load_json(args.evidence), artifact_root=args.artifacts,
                                    repository=args.repository)
    except EvidenceError as error:
        print(json.dumps({"status": "refused", "reason": str(error)}))
        return 1
    print(json.dumps({"status": "pass", **verdict}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

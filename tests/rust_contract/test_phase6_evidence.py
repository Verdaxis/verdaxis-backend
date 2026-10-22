"""Synthetic, source-bound evidence for the stream acceptance gate.

Each test builds real Git repositories, wrapper logs in the adapter format and a
committed inventory that carries the proposed owner fields (repository, runner,
test path and name, support paths, database suite and RED kinds). A complete bundle
passes; every refusal test changes one property and asserts the specific reason.
"""
from __future__ import annotations

import copy
import hashlib
import importlib
import inspect
import json
import os
import re
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
INVENTORY_PATH = "tests/rust_contract/phase6/required-scenarios.json"
REAL_INVENTORY = json.loads((ROOT / INVENTORY_PATH).read_text())
SUITES = set(REAL_INVENTORY["postgres_suites"])
EMPTY_DIFF = hashlib.sha256(b"").hexdigest()
IMAGE = "postgis/postgis:17-3.6-alpine@sha256:" + "4" * 64
SUPPORT_PATH = "tests/postgres/retention_test_support.py"
RACES_PATH = "tests/postgres/test_phase6_races.py"
# The PostgreSQL suite that runs each Rust target against the disposable database.
RUST_SUITES = {
    "streams_store": "tests/postgres/test_phase6_retention.py",
    "streams_auth": "tests/postgres/test_phase6_authorization.py",
    "streams_election": "tests/postgres/test_phase6_election.py",
    "streams_dispatch": "tests/postgres/test_phase6_dispatch.py",
    "streams_replay": "tests/postgres/test_phase6_replay.py",
    "streams_http": "tests/postgres/test_phase6_http.py",
    "streams_runtime": "tests/postgres/test_phase6_runtime.py",
    "streams_watchlist_guard": "tests/postgres/test_phase6_watchlist_guard.py",
}
TEST_RUNNERS = {"cargo", "pytest", "vitest", "journey"}
# One private mutation of the implementation file, used by the negative-control RED.
CONTROL_PATCH = (b"diff --git a/implementation.txt b/implementation.txt\n"
                 b"--- a/implementation.txt\n+++ b/implementation.txt\n"
                 b"@@ -1 +1 @@\n-complete\n+mutated\n")


def owner_fields(case):
    """Add the proposed owner fields to one real inventory scenario."""
    target = case["target"]
    if target.startswith("streams_"):
        fields = {"repository": "backend", "runner": "cargo",
                  "test_path": f"rs/read/tests/{target}.rs", "test_name": case["test"],
                  "red": ["assertion", "missing_interface"],
                  "missing_interface": {"code": "E0432", "symbol": "verdaxis_read::streams::bus"}}
        if target in RUST_SUITES:
            fields["suite"] = RUST_SUITES[target]
        if target == "streams_store":
            fields["support_paths"] = [SUPPORT_PATH]
    elif target.startswith("tests/"):
        name = case["test"] if case["test"].startswith("test_") else f'test_{case["test"]}'
        fields = {"repository": "backend", "runner": "pytest", "test_path": target,
                  "test_name": name, "red": ["assertion"]}
    elif target == "frontend J profile":
        fields = {"repository": "frontend", "runner": "journey",
                  "test_path": "scripts/journeys/phase6.py", "test_name": case["id"],
                  "red": ["assertion"]}
    elif target == "frontend contract suites":
        fields = {"repository": "frontend", "runner": "vitest",
                  "test_path": "src/tests/use-sse.test.tsx",
                  "test_name": f'phase6 > {case["test"]}', "red": ["assertion"]}
    elif target == "independent review":
        fields = {"repository": "backend", "runner": "review", "red": []}
    else:
        fields = {"repository": "backend", "runner": "gate", "red": []}
    if case["task"] == "10":
        fields["red"] = ["negative_control"]
    if case["task"] == "14b":
        fields["red"] = ["assertion", "negative_control"]
    return {**case, **fields}


CASES = [owner_fields(case) for case in REAL_INVENTORY["scenarios"]]
OWNER_INVENTORY = {**REAL_INVENTORY, "scenarios": CASES}


def is_database(case):
    return case.get("test_path") in SUITES or "suite" in case


def has_test_runner(case):
    return case["runner"] in TEST_RUNNERS


def by_task(cases):
    tasks = {}
    for case in cases:
        tasks.setdefault(case["task"], []).append(case)
    return tasks


def first(predicate):
    return next(case["id"] for case in CASES if predicate(case))


def backend_test(case):
    return case["repository"] == "backend" and has_test_runner(case)


def backend_unit(case):
    return backend_test(case) and not is_database(case)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def commit(repo, subject, files):
    for path, text in files.items():
        target = repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    git(repo, "add", "-A")
    git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
        "-c", "commit.gpgsign=false", "commit", "-qm", subject)
    return git(repo, "rev-parse", "HEAD")


def new_repository(path):
    path.mkdir()
    git(path, "init", "-q")
    return path


def outcome_line(case, outcome):
    """The line that the case's runner prints for one outcome."""
    runner, path, name = case["runner"], case.get("test_path"), case.get("test_name")
    if runner == "cargo":
        marker = {"passed": "ok", "failed": "FAILED",
                  "skipped": "ignored, requires the disposable PostgreSQL harness"}[outcome]
        context = f"CARGO_PACKAGE_ROOT=rs/read\n     Running tests/{Path(path).name} (target/debug/deps/fixture-0)"
        diagnostic = f"\n---- {name} stdout ----\nthread '{name}' panicked at {path}:1:1:\nassertion `left == right` failed" if outcome == "failed" else ""
        return f"{context}\ntest {name} ... {marker}{diagnostic}"
    if runner == "pytest":
        if outcome == "skipped":
            return f"{path}::{name} SKIPPED (synthetic)"
        suffix = " - AssertionError: synthetic" if outcome == "failed" else ""
        marker = {"passed": "PASSED", "failed": "FAILED", "error": "ERROR"}[outcome]
        return f"{marker} {path}::{name}{suffix}"
    if runner == "vitest":
        marker = {"passed": "✓", "failed": "×", "skipped": "↓"}[outcome]
        diagnostic = f"\n FAIL  {path} > {name}\nAssertionError: synthetic" if outcome == "failed" else ""
        return f" {marker} {path} > {name} 3ms{diagnostic}"
    return f'  [{"PASS" if outcome == "passed" else "FAIL"}] {path}::{name} {"AssertionError: synthetic" if outcome == "failed" else "synthetic"}'


class Clock:
    """Strictly increasing UTC stamps, one minute apart."""

    def __init__(self):
        self.minutes = 0

    def __call__(self):
        hours, minutes = divmod(self.minutes, 60)
        self.minutes += 1
        return f"2026-10-22T{8 + hours:02d}:{minutes:02d}:00Z"


class World:
    """Repositories, private artifacts and one complete evidence bundle."""

    def __init__(self, root, *, inventory=OWNER_INVENTORY):
        self.root = root
        self.artifacts = root / "artifacts"
        self.artifacts.mkdir()
        self.backend = new_repository(root / "backend")
        self.frontend = new_repository(root / "frontend")
        inventory_bytes = (json.dumps(inventory, indent=2) + "\n").encode()
        self.inventory_sha256 = digest(inventory_bytes)
        self.sha = {"base": commit(self.backend, "docs: start", {
            "README.md": "synthetic\n", INVENTORY_PATH: inventory_bytes.decode()})}
        backend_paths = {case["test_path"] for case in CASES if backend_test(case)}
        frontend_paths = {case["test_path"] for case in CASES
                          if case["repository"] == "frontend"}
        self.sha["red"] = commit(self.backend, "test: define cases", {
            path: f"synthetic test {path}\n" for path in backend_paths | {SUPPORT_PATH}})
        self.sha["impl"] = commit(self.backend, "feat: implement cases",
                                  {"implementation.txt": "complete\n"})
        commit(self.frontend, "docs: start", {"README.md": "synthetic\n"})
        self.sha["fe_red"] = commit(self.frontend, "test: define cases", {
            path: f"synthetic test {path}\n" for path in frontend_paths})
        self.sha["fe_impl"] = commit(self.frontend, "feat: implement cases",
                                     {"implementation.txt": "complete\n"})
        self.control_sha = self.sha["impl"]
        self.patch = CONTROL_PATCH
        self.render()

    # Building logs.

    def tree(self, repo, sha):
        return git(repo, "rev-parse", f"{sha}^{{tree}}")

    def applied_digest(self):
        index = self.root / "control-index"
        env = dict(os.environ, GIT_INDEX_FILE=str(index))
        def indexed(*args, data=None):
            return subprocess.run(["git", "-C", str(self.backend), *args], input=data,
                                  env=env, check=True, capture_output=True).stdout
        indexed("read-tree", self.control_sha)
        indexed("apply", "--cached", data=self.patch)
        return digest(indexed("diff", "--cached", "--binary", "--full-index", "--no-ext-diff",
                              "--no-textconv", self.control_sha))

    def source(self, repo, sha, *, diff=EMPTY_DIFF, control=None):
        lines = [f"SOURCE_SHA={sha}", f"SOURCE_TREE={self.tree(repo, sha)}",
                 f"SOURCE_DIFF_SHA256={diff}", "SOURCE_UNTRACKED=0"]
        if control:
            lines.append(f"CONTROL_PATCH_SHA256={control}")
        return lines

    def section(self, name, exit_code, body, *, database=False):
        lines = [f"GATE={name} UTC={self.clock()}", f"COMMAND=fixture {name.lower()}"]
        if database:
            lines.append(f"Starting {IMAGE} as verdaxis-analytics-test-1-2")
        return [*lines, *body, f"{name}_EXIT={exit_code} UTC={self.clock()}"]

    def write_log(self, name, text, repository=None):
        data = text.encode()
        (self.artifacts / name).write_bytes(data)
        for entry in self.bundle.get("logs", []):
            if entry["path"] == name:
                entry["sha256"] = digest(data)
                return
        self.bundle.setdefault("logs", []).append(
            {"path": name, "sha256": digest(data), "repository": repository})

    def add_log(self, name, host, repository, lines):
        repo = self.backend if repository == "backend" else self.frontend
        header = f"== verdaxis-{host}-test: rust-p6-gate at {git(repo, 'rev-parse', 'HEAD')}"
        self.write_log(name, "\n".join([f"{header} (host fixture)", *lines, "CALL_EXIT=0"])
                       + "\n", repository)

    def render(self, *, red_after_green=False, green_sha=None):
        """Write every log, artifact and the bundle for the current commits."""
        self.clock = Clock()
        self.bundle = {
            "version": 2, "scope": "cutover-rehearsal", "source_sha": self.sha["impl"],
            "release_sha": self.sha["impl"], "frontend_sha": self.sha["fe_impl"],
            "inventory_sha256": self.inventory_sha256, "database_fixture": IMAGE,
            "logs": [], "tasks": [], "implementer": "builder",
            "publication": {"requested": False},
        }
        steps = [self.render_green, self.render_red]
        for step in (steps if red_after_green else reversed(steps)):
            step(green_sha or self.sha["impl"])
        self.render_review()
        self.render_tasks()

    def render_red(self, _):
        backend = [case for case in CASES if backend_test(case)]
        unit = by_task(case for case in backend if not is_database(case))
        lines = self.source(self.backend, self.sha["red"])
        for task, cases in unit.items():
            failing = [outcome_line(case, "failed") for case in cases]
            if task == "1":
                half = len(cases) // 2
                passing = [outcome_line(case, "passed") for case in cases]
                lines += self.section("RED_T1A", 1, failing[:half] + passing[half:])
                lines += self.section("RED_T1B", 1, passing[:half] + failing[half:])
            elif task == "3":
                lines += self.section("RED_T3", 101, [
                    "   Compiling verdaxis-read v0.1.0",
                    "error[E0432]: unresolved import `verdaxis_read::streams::bus`",
                    "error: could not compile `verdaxis-read` due to 1 previous error"])
            else:
                extra = "PASSED tests/rust_contract/test_extra.py::test_extra_helper"
                lines += self.section(f"RED_T{task.upper()}", 1, failing + [extra])
        self.add_log("backend-red-m1.log", "m1", "backend", lines)
        lines = self.source(self.backend, self.sha["red"])
        for task, cases in by_task(case for case in backend if is_database(case)).items():
            if task != "10":
                lines += self.section(f"RED_DB_T{task.upper()}", 1,
                                      [outcome_line(case, "failed") for case in cases],
                                      database=True)
        self.add_log("backend-red-staging.log", "staging", "backend", lines)
        vitest = [case for case in CASES if case["runner"] == "vitest"]
        journeys = [case for case in CASES if case["runner"] == "journey"]
        self.add_log("frontend-red-m1.log", "m1", "frontend", [
            *self.source(self.frontend, self.sha["fe_red"]),
            *self.section("RED_VITEST", 1, [outcome_line(case, "failed") for case in vitest])])
        self.add_log("frontend-journeys-red.log", "prod", "frontend", [
            *self.source(self.frontend, self.sha["fe_red"]),
            *self.section("RED_JOURNEYS", 1,
                          [outcome_line(case, "failed") for case in journeys])])

    def render_green(self, candidate):
        backend = [case for case in CASES if backend_test(case)]
        control = digest(self.patch)
        (self.artifacts / "control-t10.patch").write_bytes(self.patch)
        self.add_log("backend-control-staging.log", "staging", "backend", [
            *self.source(self.backend, self.control_sha, diff=self.applied_digest(),
                         control=control),
            *self.section("CONTROL_T10", 1, [outcome_line(case, "failed")
                                             for case in CASES if case["task"] == "10"],
                          database=True)])
        unit = [outcome_line(case, "passed") for case in backend if not is_database(case)]
        self.add_log("backend-green-m1.log", "m1", "backend", [
            *self.source(self.backend, candidate),
            *self.section("GREEN_UNIT", 0, unit + [
                "PASSED tests/rust_contract/test_extra.py::test_extra_helper",
                "test helper_without_database ... ignored, requires the disposable harness"])])
        suites = sorted(SUITES)
        database = [outcome_line(case, "passed") for case in backend if is_database(case)]
        components = [f"PASSED {suite}::test_rust_component" for suite in suites]
        self.add_log("backend-green-staging.log", "staging", "backend", [
            *self.source(self.backend, candidate),
            *self.section("GREEN_DB", 0, components + database, database=True)])
        vitest = [case for case in CASES if case["runner"] == "vitest"]
        journeys = [case for case in CASES if case["runner"] == "journey"]
        self.add_log("frontend-green-m1.log", "m1", "frontend", [
            *self.source(self.frontend, self.sha["fe_impl"]),
            *self.section("GREEN_VITEST", 0, [outcome_line(case, "passed") for case in vitest]
                          + [" ✓ src/tests/public-market-sync.test.tsx > extra > helper 2ms"])])
        self.add_log("frontend-journeys-green.log", "prod", "frontend", [
            *self.source(self.frontend, self.sha["fe_impl"]),
            *self.section("GREEN_JOURNEYS", 0,
                          [outcome_line(case, "passed") for case in journeys])])
        # Tests outside the inventory: two unit helpers, the suite components, one Vitest helper.
        self.extra_results = 2 + len(suites) + 1

    def render_review(self):
        text = (f"# Review\n\n## Verdict: accept\n\nReviewed backend {self.sha['impl']} "
                f"and frontend {self.sha['fe_impl']}.\n")
        self.write_review(text)

    def write_review(self, text):
        data = text.encode()
        (self.artifacts / "review.md").write_bytes(data)
        self.bundle["review"] = {"path": "review.md", "sha256": digest(data),
                                 "reviewer": "reviewer", "verdict": "accept"}

    def render_tasks(self):
        tasks = {}
        for case in CASES:
            if case["red"] and case["task"] not in tasks:
                tasks[case["task"]] = self.red_records(case["task"])
        self.bundle["tasks"] = list(tasks.values())

    def red_records(self, task):
        cases = [case["id"] for case in CASES if case["task"] == task]
        sample = next(case for case in CASES if case["task"] == task)
        frontend = sample["repository"] == "frontend"
        record = {"task": task,
                  "implementation_sha": self.sha["fe_impl" if frontend else "impl"]}
        if task == "1":
            half = len(cases) // 2
            record["red"] = [
                {"run": "backend-red-m1.log#RED_T1A", "kind": "assertion", "cases": cases[:half]},
                {"run": "backend-red-m1.log#RED_T1B", "kind": "assertion", "cases": cases[half:]}]
        elif task == "3":
            record["red"] = [{"run": "backend-red-m1.log#RED_T3", "kind": "missing_interface",
                              "cases": cases}]
        elif task == "10":
            record = {"task": task, "red": [{
                "run": "backend-control-staging.log#CONTROL_T10", "kind": "negative_control",
                "cases": cases,
                "patch": {"path": "control-t10.patch", "sha256": digest(self.patch)}}]}
        elif task == "12":
            vitest = [case["id"] for case in CASES if case["runner"] == "vitest"]
            journeys = [case["id"] for case in CASES if case["runner"] == "journey"]
            record["red"] = [
                {"run": "frontend-red-m1.log#RED_VITEST", "kind": "assertion", "cases": vitest},
                {"run": "frontend-journeys-red.log#RED_JOURNEYS", "kind": "assertion",
                 "cases": journeys}]
        else:
            log = "backend-red-staging.log#RED_DB_T" if is_database(sample) \
                else "backend-red-m1.log#RED_T"
            record["red"] = [{"run": f"{log}{task.upper()}", "kind": "assertion",
                              "cases": cases}]
        return record

    # Changing evidence.

    def lines(self, log):
        return (self.artifacts / log).read_text().split("\n")

    def edit(self, log, old, new=()):
        """Replace one whole line of a log with zero or more lines and re-pin its digest."""
        lines = self.lines(log)
        text = "\n".join(lines)
        assert text.count(old) == 1, old
        replacement = new if isinstance(new, str) else "\n".join(new)
        text = text.replace(old, replacement, 1)
        lines = text.split("\n")
        self.write_log(log, "\n".join(lines))

    def line_starting(self, log, prefix):
        matches = [line for line in self.lines(log) if line.startswith(prefix)]
        assert len(matches) == 1, prefix
        return matches[0]

    def set_exit(self, log, gate_name, code):
        """Change the exit code of one gate section and keep its finish stamp."""
        line = self.line_starting(log, f"{gate_name}_EXIT=")
        self.edit(log, line, re.sub(r"_EXIT=-?\d+", f"_EXIT={code}", line, count=1))

    def sections(self):
        return sum(line.startswith("GATE=") for path in self.artifacts.glob("*.log")
                   for line in path.read_text().split("\n"))

    def task(self, task):
        return next(record for record in self.bundle["tasks"] if record["task"] == task)

    def case(self, case_id):
        return next(case for case in CASES if case["id"] == case_id)


@pytest.fixture
def world(tmp_path):
    return World(tmp_path)


def gate():
    module_path = ROOT / "scripts/rust_contract/phase6_evidence.py"
    assert module_path.is_file(), "stream evidence gate is not implemented"
    module = importlib.import_module("scripts.rust_contract.phase6_evidence")
    parameters = inspect.signature(module.validate_evidence).parameters
    assert "repositories" in parameters, "stream evidence gate does not read host logs"
    return module


def validate(world):
    return gate().validate_evidence(
        world.bundle, artifact_root=world.artifacts,
        repositories={"backend": world.backend, "frontend": world.frontend})


def passes(world):
    module = gate()
    try:
        return validate(world)
    except module.EvidenceError as error:
        raise AssertionError(f"evidence refused: {error}") from error


def refused(world, reason):
    module = gate()
    with pytest.raises(module.EvidenceError) as caught:
        validate(world)
    assert str(caught.value) == reason


# Complete evidence.

def test_evidence_complete_bundle_passes(world):
    assert passes(world) == {
        "scope": "cutover-rehearsal", "source_sha": world.sha["impl"],
        "frontend_sha": world.sha["fe_impl"], "inventory_sha256": world.inventory_sha256,
        "scenarios": len(CASES), "runs": world.sections(),
        "extra_results": world.extra_results,
        "verdict_cases": ["E198", "E199", "E200", "E201"], "publication": "not-requested",
    }


def test_evidence_candidate_does_not_require_cutover_cases(world):
    world.bundle["scope"] = "candidate"
    world.bundle["tasks"] = [record for record in world.bundle["tasks"]
                             if record["task"] != "14b"]
    candidate_cases = [case for case in CASES if "candidate" in case["scopes"]]
    assert passes(world)["scenarios"] == len(candidate_cases)


def test_evidence_cutover_red_outside_candidate_scope_refused(world):
    world.bundle["scope"] = "candidate"
    refused(world, f'RED case not in scope: {first(lambda case: case["task"] == "14b")}')


# The named scenario requirements (E186-E197).

def test_evidence_missing_case_refused(world):
    world.edit("backend-green-staging.log", outcome_line(world.case("R01"), "passed"))
    refused(world, "candidate pass missing: R01")


def test_evidence_skipped_database_refused(world):
    world.edit("backend-green-staging.log",
               f"Starting {IMAGE} as verdaxis-analytics-test-1-2")
    refused(world, f"database run required: {first(is_database)}")


def test_evidence_missing_leader_direction_refused(world):
    world.edit("backend-green-staging.log", outcome_line(world.case("E129"), "passed"))
    refused(world, "candidate pass missing: E129")


def test_evidence_missing_journey_refused(world):
    world.edit("frontend-journeys-green.log", outcome_line(world.case("J61"), "passed"))
    refused(world, "candidate pass missing: J61")


def test_evidence_missing_review_refused(world):
    del world.bundle["review"]
    refused(world, "review record required")


def test_evidence_missing_drain_refused(world):
    world.edit("backend-green-staging.log", outcome_line(world.case("E183"), "passed"))
    refused(world, "candidate pass missing: E183")


def test_evidence_missing_exit_refused(world):
    log = "backend-green-staging.log"
    world.edit(log, world.line_starting(log, "GREEN_DB_EXIT="))
    refused(world, "gate section not closed")


def test_evidence_missing_call_exit_refused(world):
    world.edit("backend-green-staging.log", "CALL_EXIT=0")
    refused(world, "terminal exit marker required")


def test_evidence_identity_mismatch_refused(world):
    world.bundle["release_sha"] = world.sha["red"]
    refused(world, "release identity mismatch")


def test_evidence_missing_red_refused(world):
    world.bundle["tasks"].remove(world.task("8"))
    refused(world, "RED evidence missing: R01")


def test_evidence_red_after_implementation_refused(world):
    cases = [case for case in CASES if case["task"] == "15a"]
    world.add_log("late-red-m1.log", "m1", "backend", [
        *world.source(world.backend, world.sha["impl"]),
        *world.section("LATE_RED", 1, [outcome_line(case, "failed") for case in cases])])
    world.task("15a")["red"][0]["run"] = "late-red-m1.log#LATE_RED"
    refused(world, "RED must precede implementation")


def test_evidence_red_tree_mismatch_refused(world):
    log = "backend-red-m1.log"
    world.edit(log, world.line_starting(log, "SOURCE_TREE="),
               f'SOURCE_TREE={world.tree(world.backend, world.sha["impl"])}')
    refused(world, "source tree mismatch")


def test_evidence_unreleased_push_refused(world):
    world.bundle["publication"]["requested"] = True
    refused(world, "publication base required")


# GREEN: candidate pass on a clean checkout, in a run that may prove the case.

def test_evidence_stale_green_refused(world):
    world.render(green_sha=world.sha["red"])
    refused(world, "candidate pass missing: R01")


def test_evidence_failed_candidate_run_refused(world):
    log = "backend-green-m1.log"
    extra = "PASSED tests/rust_contract/test_extra.py::test_extra_helper"
    world.edit(log, extra, "FAILED tests/rust_contract/test_extra.py::test_extra_helper")
    world.set_exit(log, "GREEN_UNIT", 1)
    refused(world, f"candidate run failed: {first(backend_unit)}")


def test_evidence_dirty_green_refused(world):
    log = "backend-green-m1.log"
    world.edit(log, "SOURCE_UNTRACKED=0", "SOURCE_UNTRACKED=1")
    refused(world, f"clean source state required: {first(backend_unit)}")


def test_evidence_skipped_required_case_refused(world):
    case = world.case(first(lambda case: case["runner"] == "cargo" and not is_database(case)))
    world.edit("backend-green-m1.log", outcome_line(case, "passed"),
               outcome_line(case, "skipped"))
    refused(world, f'required case skipped: {case["id"]}')


def test_evidence_test_identity_bound_to_inventory_path(world):
    case = world.case("E186")
    world.edit("backend-green-m1.log", outcome_line(case, "passed"),
               f'PASSED tests/rust_contract/test_other.py::{case["test_name"]}')
    refused(world, "candidate pass missing: E186")


def test_evidence_database_suite_required(world):
    world.edit("backend-green-staging.log",
               f'PASSED {world.case("R01")["suite"]}::test_rust_component')
    refused(world, "database suite not run: R01")


def test_evidence_journey_host_required(world):
    log = "frontend-journeys-green.log"
    header = world.lines(log)[0]
    world.edit(log, header, header.replace("verdaxis-prod-test", "verdaxis-m1-test"))
    refused(world, "journey host required: J61")


# RED: kind, exit, ancestry, order and unchanged test source.

def test_evidence_red_after_green_refused(world):
    world.render(red_after_green=True)
    refused(world, "GREEN must follow RED: R01")


def test_evidence_red_off_branch_refused(world):
    git(world.backend, "checkout", "-q", "-b", "side", world.sha["red"])
    side = commit(world.backend, "test: side change", {"side.txt": "side\n"})
    log = "backend-red-m1.log"
    world.edit(log, f'SOURCE_SHA={world.sha["red"]}', f"SOURCE_SHA={side}")
    world.edit(log, world.line_starting(log, "SOURCE_TREE="),
               f"SOURCE_TREE={world.tree(world.backend, side)}")
    refused(world, "RED source not on candidate ancestry")


def test_evidence_red_test_source_change_refused(world):
    git(world.backend, "checkout", "-q", world.sha["impl"])
    world.sha["impl"] = commit(world.backend, "test: adjust races",
                               {RACES_PATH: "changed synthetic test\n"})
    world.control_sha = world.sha["impl"]
    world.render()
    refused(world, "RED and GREEN test source mismatch: "
                   f'{first(lambda case: case.get("test_path") == RACES_PATH)}')


def test_evidence_red_support_source_change_refused(world):
    git(world.backend, "checkout", "-q", world.sha["impl"])
    world.sha["impl"] = commit(world.backend, "test: adjust support",
                               {SUPPORT_PATH: "changed support\n"})
    world.control_sha = world.sha["impl"]
    world.render()
    refused(world, "RED and GREEN test source mismatch: "
                   f'{first(lambda case: SUPPORT_PATH in case.get("support_paths", []))}')


def test_evidence_red_kind_must_be_allowed(world):
    cases = [case for case in CASES if case["task"] == "10"]
    world.add_log("assertion-t10.log", "staging", "backend", [
        *world.source(world.backend, world.sha["red"]),
        *world.section("RED_DB_T10", 1, [outcome_line(case, "failed") for case in cases],
                       database=True)])
    world.task("10").update(implementation_sha=world.sha["impl"], red=[
        {"run": "assertion-t10.log#RED_DB_T10", "kind": "assertion",
         "cases": [case["id"] for case in cases]}])
    refused(world, f'RED kind not allowed: {cases[0]["id"]}')


def test_evidence_red_compile_failure_requires_cargo_exit(world):
    world.set_exit("backend-red-m1.log", "RED_T3", 1)
    refused(world, "RED compile failure required")


def test_evidence_red_compile_failure_without_test_results(world):
    world.edit("backend-red-m1.log",
               "error: could not compile `verdaxis-read` due to 1 previous error",
               "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured")
    refused(world, "RED compile failure required")


def test_evidence_red_error_is_not_an_assertion(world):
    case = world.case("E186")
    world.edit("backend-red-m1.log", outcome_line(case, "failed"), outcome_line(case, "error"))
    refused(world, "RED assertion missing: E186")


def test_evidence_red_requires_test_failure_exit(world):
    world.set_exit("backend-red-m1.log", "RED_T15A", 2)
    refused(world, "RED failure exit required")


def test_evidence_red_database_run_required(world):
    log = "backend-red-staging.log"
    lines = world.lines(log)
    marker = lines.index(world.line_starting(log, "GATE=RED_DB_T8 ")) + 2
    assert lines[marker].startswith("Starting ")
    del lines[marker]
    world.write_log(log, "\n".join(lines))
    refused(world, "RED database run required: R01")


def test_evidence_dirty_red_refused(world):
    world.edit("backend-red-m1.log", "SOURCE_UNTRACKED=0", "SOURCE_UNTRACKED=2")
    refused(world, "clean RED source state required")


def test_evidence_unknown_run_reference_refused(world):
    world.task("8")["red"][0]["run"] = "backend-red-staging.log#ABSENT"
    refused(world, "unknown run reference")


# Negative controls: one private mutation per run, never committed.

def test_evidence_control_patch_digest_required(world):
    log = "backend-control-staging.log"
    world.edit(log, world.line_starting(log, "CONTROL_PATCH_SHA256="),
               "CONTROL_PATCH_SHA256=" + "5" * 64)
    refused(world, "control patch not applied in run")


def test_evidence_control_requires_modified_tree(world):
    log = "backend-control-staging.log"
    world.edit(log, world.line_starting(log, "SOURCE_DIFF_SHA256="),
               f"SOURCE_DIFF_SHA256={EMPTY_DIFF}")
    refused(world, "control patch not applied in run")


def test_evidence_one_mutation_per_run(world):
    log = "backend-control-staging.log"
    line = world.line_starting(log, "CONTROL_PATCH_SHA256=")
    world.edit(log, line, [line, "CONTROL_PATCH_SHA256=" + "6" * 64])
    refused(world, "one control mutation per run")


def test_evidence_control_patch_must_match_source(world):
    other = world.patch.replace(b"-complete", b"-different")
    (world.artifacts / "control-t10.patch").write_bytes(other)
    world.task("10")["red"][0]["patch"]["sha256"] = digest(other)
    log = "backend-control-staging.log"
    world.edit(log, world.line_starting(log, "CONTROL_PATCH_SHA256="),
               f"CONTROL_PATCH_SHA256={digest(other)}")
    refused(world, "control patch does not match source")


def test_evidence_committed_control_refused(world):
    git(world.backend, "checkout", "-q", world.sha["impl"])
    world.sha["impl"] = commit(world.backend, "feat: change implementation",
                               {"implementation.txt": "mutated\n"})
    world.render()
    refused(world, "control mutation is committed")


# Inventory, review, publication and artifact boundaries.

def test_evidence_inventory_digest_pinned(world):
    world.bundle["inventory_sha256"] = "0" * 64
    refused(world, "inventory identity mismatch")


def test_evidence_inventory_owner_fields_required(tmp_path):
    refused(World(tmp_path, inventory=REAL_INVENTORY), "inventory test identity incomplete")


def test_evidence_review_verdict_must_accept(world):
    world.write_review(f'## Verdict: fix\n{world.sha["impl"]} {world.sha["fe_impl"]}\n')
    world.bundle["review"]["verdict"] = "accept"
    refused(world, "accepting review verdict required")


def test_evidence_review_must_name_candidate(world):
    world.write_review(f'## Verdict: accept\nReviewed {world.sha["red"]}.\n')
    refused(world, "review must name the candidate")


def test_evidence_self_review_refused(world):
    world.bundle["review"]["reviewer"] = " Builder "
    refused(world, "independent reviewer required")


def test_evidence_publication_exact_ancestry_required(world):
    publication = world.bundle["publication"]
    exact = git(world.backend, "rev-list", f'{world.sha["red"]}..{world.sha["impl"]}').split()
    publication.update(requested=True, base_sha=world.sha["red"], approved_shas=exact,
                       reviewed_shas=exact, approver="PM")
    assert passes(world)["publication"] == "approved"
    publication["approved_shas"] = []
    refused(world, "exact publication ancestry required")


def test_evidence_publication_approver_independent(world):
    exact = git(world.backend, "rev-list", f'{world.sha["red"]}..{world.sha["impl"]}').split()
    world.bundle["publication"].update(requested=True, base_sha=world.sha["red"],
                                       approved_shas=exact, reviewed_shas=exact,
                                       approver="BUILDER")
    refused(world, "independent publication approver required")


def test_evidence_frontend_candidate_required(world):
    del world.bundle["frontend_sha"]
    refused(world, "frontend candidate required")


def test_evidence_artifact_parent_path_refused(world):
    world.bundle["logs"][0]["path"] = "../outside.log"
    refused(world, "relative artifact path required")


def test_evidence_artifact_escape_refused(world):
    entry = world.bundle["logs"][0]
    outside = world.root / "outside.log"
    outside.write_bytes((world.artifacts / entry["path"]).read_bytes())
    (world.artifacts / "link.log").symlink_to(outside)
    entry["path"] = "link.log"
    refused(world, "artifact outside evidence root")


def test_evidence_artifact_digest_required(world):
    world.bundle["logs"][0]["sha256"] = "0" * 64
    refused(world, "artifact digest mismatch")


def test_evidence_duplicate_log_refused(world):
    world.bundle["logs"].append(copy.deepcopy(world.bundle["logs"][0]))
    refused(world, "duplicate host log")


def test_evidence_log_order_refused(world):
    log = "backend-green-m1.log"
    world.edit(log, world.line_starting(log, "GREEN_UNIT_EXIT="),
               "GREEN_UNIT_EXIT=0 UTC=2026-10-22T07:00:00Z")
    refused(world, "log stamps out of order")


def test_evidence_shallow_repository_refused(world, tmp_path):
    shallow = tmp_path / "shallow"
    subprocess.run(["git", "clone", "-q", "--depth", "1", f"file://{world.backend}",
                    str(shallow)], check=True, capture_output=True)
    module = gate()
    with pytest.raises(module.EvidenceError) as caught:
        module.validate_evidence(world.bundle, artifact_root=world.artifacts,
                                 repositories={"backend": shallow, "frontend": world.frontend})
    assert str(caught.value) == "complete source history required"


def test_evidence_malformed_value_refused(world):
    world.bundle["logs"][0]["path"] = "bad\0name.log"
    refused(world, "relative artifact path required")


def test_evidence_cli_reports_refusal(world, capsys):
    path = world.root / "bundle.json"
    path.write_text('{"version": 2, "version": 2}')
    assert gate().main([str(path), "--artifacts", str(world.artifacts),
                        "--repository", str(world.backend)]) == 1
    assert json.loads(capsys.readouterr().out) == {"status": "refused",
                                                   "reason": "duplicate JSON field"}


def test_evidence_cli_reports_pass(world, capsys):
    path = world.root / "bundle.json"
    path.write_text(json.dumps(world.bundle))
    assert gate().main([str(path), "--artifacts", str(world.artifacts),
                        "--repository", str(world.backend),
                        "--frontend-repository", str(world.frontend)]) == 0
    output = json.loads(capsys.readouterr().out)
    assert output["status"] == "pass" and output["source_sha"] == world.sha["impl"]


# Source context and diagnostic contracts.

def test_evidence_cargo_wrong_target_refused(world):
    case = world.case("E032")
    log = "backend-green-m1.log"
    old = outcome_line(case, "passed")
    world.edit(log, old, old.replace("tests/streams_bus.rs", "tests/other.rs"))
    refused(world, "candidate pass missing: E032")


def test_evidence_cargo_missing_context_refused(world):
    case = world.case("E032")
    world.edit("backend-green-m1.log", outcome_line(case, "passed"),
               f'CARGO_PACKAGE_ROOT=rs/read\ntest {case["test_name"]} ... ok')
    refused(world, "cargo source context required")


def test_evidence_journey_wrong_path_refused(world):
    case = world.case("J61")
    old = outcome_line(case, "passed")
    world.edit("frontend-journeys-green.log", old, old.replace(case["test_path"], "other.py"))
    refused(world, "candidate pass missing: J61")


@pytest.mark.parametrize("diagnostic", [
    "error[E0308]: mismatched types", "error[E0502]: cannot borrow `value`",
    "error[E0432]: unresolved import `unrelated::thing`"])
def test_evidence_unrelated_compile_error_refused(world, diagnostic):
    world.edit("backend-red-m1.log",
               "error[E0432]: unresolved import `verdaxis_read::streams::bus`", diagnostic)
    refused(world, "RED missing interface diagnostic required: T01")


def test_evidence_nonassertion_exception_refused(world):
    case = world.case("E186")
    world.edit("backend-red-m1.log", outcome_line(case, "failed"),
               f'FAILED {case["test_path"]}::{case["test_name"]} - RuntimeError: setup')
    refused(world, "RED assertion missing: E186")


def test_evidence_cargo_nonassertion_panic_refused(world):
    case = next(case for case in CASES if case["task"] == "4")
    old = outcome_line(case, "failed")
    world.edit("backend-red-staging.log", old,
               old.replace("assertion `left == right` failed", "explicit panic"))
    refused(world, f'RED assertion missing: {case["id"]}')


def test_evidence_control_applied_diff_mismatch_refused(world):
    log = "backend-control-staging.log"
    world.edit(log, world.line_starting(log, "SOURCE_DIFF_SHA256="),
               "SOURCE_DIFF_SHA256=" + "7" * 64)
    refused(world, "control applied diff mismatch")


def test_evidence_control_untracked_change_refused(world):
    world.edit("backend-control-staging.log", "SOURCE_UNTRACKED=0", "SOURCE_UNTRACKED=1")
    refused(world, "control untracked changes refused")


def test_evidence_control_extra_change_refused(world):
    world.patch += (b"diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n"
                    b"@@ -1 +1 @@\n-synthetic\n+changed\n")
    world.render()
    refused(world, "one implementation control required")


def test_evidence_control_test_mutation_refused(world):
    path = world.case("E122")["test_path"]
    world.patch = (f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
                   f"@@ -1 +1 @@\n-synthetic test {path}\n+mutated\n").encode()
    world.render()
    refused(world, "control protected path refused")


def test_evidence_listed_unmapped_suite_required(world):
    # Coexistence has no direct scenario mapping in this fixture.
    suite = "tests/postgres/test_phase6_coexistence.py"
    world.edit("backend-green-staging.log", f"PASSED {suite}::test_rust_component")
    refused(world, f"database suite not run: {suite}")


def test_evidence_compile_red_database_required(world):
    cases = [case for case in CASES if case["task"] == "4"]
    world.add_log("compile-red.log", "m1", "backend", [
        *world.source(world.backend, world.sha["red"]),
        *world.section("COMPILE", 101, [
            "error[E0432]: unresolved import `verdaxis_read::streams::bus`"] )])
    world.task("4")["red"] = [{"run": "compile-red.log#COMPILE",
                                "kind": "missing_interface",
                                "cases": [case["id"] for case in cases]}]
    refused(world, f'RED database run required: {cases[0]["id"]}')

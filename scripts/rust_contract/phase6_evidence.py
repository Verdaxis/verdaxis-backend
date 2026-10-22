"""Validate neutral stream acceptance evidence against source and host logs.

Inputs: a private evidence bundle, the private directory that holds the wrapper logs
and review and control files that the bundle names, and the source repositories.
The gate reads the required scenarios from the candidate commit, turns every wrapper
log into run records with `phase6_record`, and refuses the bundle unless:

- every in-scope test scenario passed, at the candidate, in a run with exit 0 on a
  clean checkout (database scenarios: a Linux run of the pinned PostgreSQL fixture;
  journeys: the prod test host);
- every scenario whose inventory entry names RED kinds has an earlier RED of an
  allowed kind on the candidate's ancestry, with the same test source bytes;
- review scenarios have an accepting review file that names the candidate.

Hashes and Git ancestry prove consistency only. Host identity, reviewer identity and
PM acceptance stay outside this gate. The gate never runs commands from evidence,
opens a database, or prints captured test output.

The bundle (version 2) has these fields:

    scope              "candidate" or "cutover-rehearsal"
    source_sha         backend candidate commit; release_sha must be equal
    frontend_sha       frontend candidate commit, when frontend scenarios are in scope
    inventory_sha256   pinned digest of the inventory file at the candidate
    database_fixture   pinned PostgreSQL image reference with its sha256 digest
    logs               [{"path", "sha256", "repository": "backend" | "frontend"}]
    tasks              [{"task", "implementation_sha", "red": [RED record]}]
    review             {"path", "sha256", "reviewer", "verdict"}
    implementer        name of the implementing lane or person
    publication        {"requested": false} or the exact approved and reviewed commits

A run is named `<log path>#<gate name>`. A RED record is
`{"run", "kind", "cases": [scenario IDs], "patch": artifact}`; `patch` is used only by
the `negative_control` kind.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from scripts.rust_contract import phase6_record

INVENTORY_PATH = "tests/rust_contract/phase6/required-scenarios.json"
BUNDLE_VERSION = 2
SCOPES = {"candidate", "cutover-rehearsal"}
REPOSITORIES = {"backend", "frontend"}
TEST_RUNNERS = {"cargo", "pytest", "vitest", "journey"}
RUNNERS = TEST_RUNNERS | {"review", "gate"}
RED_KINDS = {"assertion", "missing_interface", "negative_control"}
LINUX_HOSTS = {"staging", "prod"}
JOURNEY_HOST = "prod"
# pytest, Vitest and the journey harness exit 1 when a test fails; cargo test exits 101.
TEST_FAILURE_EXITS = {1, 101}
CARGO_FAILURE_EXIT = 101
ACCEPTING_VERDICT = "accept"
SHA = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
DATABASE_IMAGE = re.compile(r"\S+@sha256:[0-9a-f]{64}\Z")
VERDICT_LINE = re.compile(r"^[#*\s]*verdict[*\s]*:[*\s]*([a-z]+)", re.IGNORECASE | re.MULTILINE)
MAX_ARTIFACT_BYTES = 16 * 1024 * 1024


class EvidenceError(ValueError):
    """A bounded, neutral rejection reason, without private record contents."""


def require(condition, reason):
    if not condition:
        raise EvidenceError(reason)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def same_person(first, second):
    return first.strip().casefold() == second.strip().casefold()


def named(value):
    return isinstance(value, str) and bool(value.strip())


def parse_json(data):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "duplicate JSON field")
            result[key] = value
        return result

    try:
        return json.loads(data, object_pairs_hook=unique_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError("JSON record unavailable") from error


def load_json(path):
    try:
        data = Path(path).read_bytes()
    except OSError as error:
        raise EvidenceError("JSON record unavailable") from error
    return parse_json(data)


def utc(value):
    return datetime.strptime(value, phase6_record.STAMP_FORMAT)


def relative_path(value):
    return (isinstance(value, str) and value and "\0" not in value
            and not Path(value).is_absolute() and ".." not in Path(value).parts)


class Source:
    """Read-only Git queries against one repository, with a clean Git environment."""

    def __init__(self, repository):
        self.repository = Path(repository)
        # Drop caller Git variables (GIT_DIR, GIT_INDEX_FILE, alternates and similar)
        # so that the repository named here is the one that is read.
        self.environment = {key: value for key, value in os.environ.items()
                            if not key.startswith("GIT_")}
        # Paths from the inventory are file names, never Git pathspec patterns.
        self.environment["GIT_LITERAL_PATHSPECS"] = "1"
        self.cache = {}
        shallow = self.git("rev-parse", "--is-shallow-repository").strip()
        grafts = self.git("rev-parse", "--git-path", "info/grafts").decode().strip()
        require(shallow == b"false" and not (self.repository / grafts).exists(),
                "complete source history required")

    def run(self, *args, data=None, environment=None):
        return subprocess.run(
            ["git", "--no-replace-objects", "-C", str(self.repository), *args],
            input=data, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=environment or self.environment, check=False,
        )

    def git(self, *args):
        result = self.run(*args)
        require(result.returncode == 0, "source verification failed")
        return result.stdout

    def cached(self, key, compute):
        if key not in self.cache:
            self.cache[key] = compute()
        return self.cache[key]

    def commit(self, sha, reason="unknown source commit"):
        require(isinstance(sha, str) and SHA.fullmatch(sha), reason)
        kind = self.cached(("type", sha), lambda: self.run("cat-file", "-t", sha).stdout.strip())
        require(kind == b"commit", reason)

    def tree(self, sha):
        self.commit(sha)
        return self.cached(("tree", sha),
                           lambda: self.git("rev-parse", f"{sha}^{{tree}}").decode().strip())

    def is_ancestor(self, before, after):
        self.commit(before)
        self.commit(after)

        def compute():
            code = self.run("merge-base", "--is-ancestor", before, after).returncode
            require(code in (0, 1), "source verification failed")
            return code == 0

        return self.cached(("ancestor", before, after), compute)

    def blob(self, sha, path):
        """Return the blob ID of a file at a commit, or None when no file has that path."""
        require(relative_path(path) and ":" not in path, "relative source path required")
        self.commit(sha)

        def compute():
            listing = self.git("ls-tree", "--full-tree", "-z", sha, "--", path)
            entries = [entry for entry in listing.split(b"\0") if entry]
            if len(entries) != 1:
                return None
            fields, _, name = entries[0].partition(b"\t")
            mode_type_object = fields.split()
            if name.decode() != path or mode_type_object[1:2] != [b"blob"]:
                return None
            return mode_type_object[2].decode()

        return self.cached(("blob", sha, path), compute)

    def read(self, sha, path):
        blob = self.blob(sha, path)
        return None if blob is None else self.git("cat-file", "blob", blob)

    def applies(self, sha, patch, *, reverse=False):
        """Whether a patch applies to the tree of a commit; no worktree is touched."""
        self.commit(sha)
        with tempfile.TemporaryDirectory() as scratch:
            environment = dict(self.environment, GIT_INDEX_FILE=str(Path(scratch) / "index"))
            loaded = self.run("read-tree", sha, environment=environment)
            require(loaded.returncode == 0, "source verification failed")
            check = ["apply", "--cached", "--check", *(["--reverse"] if reverse else [])]
            return self.run(*check, data=patch, environment=environment).returncode == 0


def read_artifact(reference, root):
    require(isinstance(reference, dict), "artifact reference required")
    name = reference.get("path")
    expected = reference.get("sha256")
    require(relative_path(name), "relative artifact path required")
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


@dataclass(frozen=True)
class Scenario:
    """One required scenario with the owner fields that bind it to a test."""

    id: str
    task: str
    scopes: frozenset
    repository: str
    runner: str
    test_path: str | None
    test_name: str | None
    support_paths: tuple
    suite: str | None
    red: frozenset
    database: bool

    @classmethod
    def from_inventory(cls, case, suites):
        require(isinstance(case, dict) and named(case.get("id")) and named(case.get("task"))
                and case.get("required") is True and isinstance(case.get("scopes"), list)
                and case["scopes"] and set(case["scopes"]) <= SCOPES,
                "required inventory invalid")
        repository, runner, red = case.get("repository"), case.get("runner"), case.get("red")
        require(repository in REPOSITORIES and runner in RUNNERS and isinstance(red, list)
                and set(red) <= RED_KINDS and len(set(red)) == len(red),
                "inventory test identity incomplete")
        test_path, test_name = case.get("test_path"), case.get("test_name")
        support, suite = case.get("support_paths", []), case.get("suite")
        if runner in TEST_RUNNERS:
            require(relative_path(test_path) and named(test_name) and isinstance(support, list)
                    and all(relative_path(path) for path in support),
                    "inventory test identity incomplete")
            require("missing_interface" not in red or runner == "cargo",
                    "required inventory invalid")
        else:
            require(not red and test_path is None and test_name is None and not support
                    and suite is None, "required inventory invalid")
        require(suite is None or suite in suites, "required inventory invalid")
        return cls(id=case["id"], task=case["task"], scopes=frozenset(case["scopes"]),
                   repository=repository, runner=runner, test_path=test_path,
                   test_name=test_name, support_paths=tuple(support), suite=suite,
                   red=frozenset(red), database=test_path in suites or suite is not None)

    @property
    def key(self):
        """The test identity as the adapter records it for this runner."""
        if self.runner == "pytest":
            return f"{self.test_path}::{self.test_name}"
        if self.runner == "vitest":
            return f"{self.test_path} > {self.test_name}"
        return self.test_name

    @property
    def source_paths(self):
        return (self.test_path, *self.support_paths)


def load_inventory(source, candidate, pinned_digest):
    require(isinstance(pinned_digest, str) and DIGEST.fullmatch(pinned_digest),
            "inventory digest required")
    data = source.read(candidate, INVENTORY_PATH)
    require(data is not None, "inventory missing at candidate")
    require(sha256(data) == pinned_digest, "inventory identity mismatch")
    document = parse_json(data)
    require(isinstance(document, dict), "required inventory invalid")
    suites, scenarios = document.get("postgres_suites"), document.get("scenarios")
    require(isinstance(suites, list) and all(relative_path(suite) for suite in suites)
            and len(set(suites)) == len(suites) and isinstance(scenarios, list) and scenarios,
            "required inventory invalid")
    cases, keys = {}, set()
    for entry in scenarios:
        case = Scenario.from_inventory(entry, set(suites))
        require(case.id not in cases, "required inventory invalid")
        cases[case.id] = case
        if case.runner in TEST_RUNNERS:
            # One test proves one scenario, so a single output line cannot cover two.
            require((case.runner, case.key) not in keys, "required inventory invalid")
            keys.add((case.runner, case.key))
    return cases


@dataclass(frozen=True)
class HostRun:
    id: str
    repository: str
    record: phase6_record.Run


class _Gate:
    def __init__(self, bundle, artifact_root, repositories):
        require(isinstance(bundle, dict) and type(bundle.get("version")) is int
                and bundle["version"] == BUNDLE_VERSION, "evidence version required")
        self.bundle = bundle
        self.scope = bundle.get("scope")
        require(self.scope in SCOPES, "evidence scope required")
        self.implementer = bundle.get("implementer")
        require(named(self.implementer), "implementer required")
        self.root = Path(artifact_root).resolve()
        backend = Source(repositories["backend"])
        candidate = bundle.get("source_sha")
        backend.commit(candidate)
        require(bundle.get("release_sha") == candidate, "release identity mismatch")
        self.inventory_sha256 = bundle.get("inventory_sha256")
        self.cases = load_inventory(backend, candidate, self.inventory_sha256)
        self.required = {key: case for key, case in self.cases.items()
                         if self.scope in case.scopes}
        self.sources = {"backend": backend}
        self.candidates = {"backend": candidate}
        if any(case.repository == "frontend" for case in self.required.values()):
            require(repositories.get("frontend") is not None
                    and isinstance(bundle.get("frontend_sha"), str),
                    "frontend candidate required")
            self.sources["frontend"] = Source(repositories["frontend"])
            self.sources["frontend"].commit(bundle["frontend_sha"])
            self.candidates["frontend"] = bundle["frontend_sha"]
        self.fixture = None
        if any(case.database for case in self.required.values()):
            self.fixture = bundle.get("database_fixture")
            require(isinstance(self.fixture, str) and DATABASE_IMAGE.fullmatch(self.fixture),
                    "database fixture identity required")
        self.runs = self.load_runs()

    def load_runs(self):
        logs = self.bundle.get("logs")
        require(isinstance(logs, list) and logs, "host logs required")
        runs, digests = {}, set()
        for entry in logs:
            require(isinstance(entry, dict) and entry.get("repository") in self.sources,
                    "log repository required")
            data = read_artifact(entry, self.root)
            require(entry["sha256"] not in digests, "duplicate host log")
            digests.add(entry["sha256"])
            try:
                records = phase6_record.parse_log(data)
            except phase6_record.LogError as error:
                raise EvidenceError(str(error)) from error
            source = self.sources[entry["repository"]]
            for record in records:
                source.commit(record.source.sha)
                require(record.source.tree in (None, source.tree(record.source.sha)),
                        "source tree mismatch")
                run_id = f'{entry["path"]}#{record.name}'
                require(run_id not in runs, "duplicate run identity")
                runs[run_id] = HostRun(run_id, entry["repository"], record)
        return runs

    def validate(self):
        green = {case.id: self.green_runs(case) for case in self.required.values()
                 if case.runner in TEST_RUNNERS}
        self.check_red(green)
        self.check_review()
        publication = self.check_publication()
        verdict = {"scope": self.scope, "source_sha": self.candidates["backend"],
                   "inventory_sha256": self.inventory_sha256,
                   "scenarios": len(self.required), "runs": len(self.runs),
                   "extra_results": self.extra_results(),
                   "verdict_cases": sorted(case.id for case in self.required.values()
                                           if case.runner == "gate"),
                   "publication": publication}
        if "frontend" in self.candidates:
            verdict["frontend_sha"] = self.candidates["frontend"]
        return verdict

    # GREEN: a pass at the candidate, in a run that is allowed to prove this case.

    def green_problem(self, case, record):
        if record.exit != 0:
            return "candidate run failed"
        if record.source.clean is not True:
            return "clean source state required"
        if case.database and (record.database != self.fixture
                              or record.host not in LINUX_HOSTS):
            return "database run required"
        if case.suite is not None and not record.suite_passed(case.suite):
            return "database suite not run"
        if case.runner == "journey" and record.host != JOURNEY_HOST:
            return "journey host required"
        return None

    def green_runs(self, case):
        candidate = self.candidates[case.repository]
        eligible, problems, skipped = [], [], False
        for run in self.runs.values():
            if run.repository != case.repository or run.record.source.sha != candidate:
                continue
            status = run.record.status(case.runner, case.key)
            skipped = skipped or status == "skipped"
            if status != "passed":
                continue
            problem = self.green_problem(case, run.record)
            if problem is None:
                eligible.append(run)
            else:
                problems.append(problem)
        if not eligible:
            if problems:
                raise EvidenceError(f"{problems[0]}: {case.id}")
            require(not skipped, f"required case skipped: {case.id}")
            raise EvidenceError(f"candidate pass missing: {case.id}")
        return eligible

    def extra_results(self):
        """Count test results in passing candidate runs that no inventory scenario names."""
        known = {(case.runner, case.key) for case in self.cases.values()
                 if case.runner in TEST_RUNNERS}
        extra = set()
        for run in self.runs.values():
            record = run.record
            if record.source.sha != self.candidates[run.repository] or record.exit != 0:
                continue
            for runner, results in record.outcomes.items():
                extra.update((run.repository, runner, key) for key in results
                             if (runner, key) not in known)
        return len(extra)

    # RED: an earlier failure of an allowed kind on the candidate's ancestry.

    def check_red(self, green):
        tasks = self.bundle.get("tasks")
        require(isinstance(tasks, list), "task records required")
        covered, names = {}, set()
        for task in tasks:
            require(isinstance(task, dict) and named(task.get("task"))
                    and task["task"] not in names, "task record required")
            names.add(task["task"])
            entries = task.get("red")
            require(isinstance(entries, list) and entries, "RED records required")
            for entry in entries:
                run, case_ids = self.red_entry(task, entry)
                for case_id in case_ids:
                    covered.setdefault(case_id, []).append(run)
        for case in self.required.values():
            if not case.red:
                continue
            require(case.id in covered, f"RED evidence missing: {case.id}")
            last_red = max(utc(run.record.finished_at) for run in covered[case.id])
            require(any(utc(run.record.started_at) > last_red for run in green[case.id]),
                    f"GREEN must follow RED: {case.id}")

    def red_entry(self, task, entry):
        """Check one RED record and return its run and the scenarios it proves."""
        require(isinstance(entry, dict), "RED record required")
        run = self.runs.get(entry.get("run"))
        require(run is not None, "unknown run reference")
        kind = entry.get("kind")
        require(kind in RED_KINDS, "RED kind required")
        case_ids = entry.get("cases")
        require(isinstance(case_ids, list) and case_ids and len(set(case_ids)) == len(case_ids),
                "RED cases required")
        record = run.record
        source = self.sources[run.repository]
        candidate = self.candidates[run.repository]
        require(source.is_ancestor(record.source.sha, candidate),
                "RED source not on candidate ancestry")
        if kind == "negative_control":
            self.check_control(entry, run)
        else:
            require(record.source.clean is True, "clean RED source state required")
            implementation = task.get("implementation_sha")
            source.commit(implementation, "implementation commit required")
            require(source.is_ancestor(implementation, candidate),
                    "implementation not on candidate ancestry")
            require(record.source.sha != implementation
                    and source.is_ancestor(record.source.sha, implementation),
                    "RED must precede implementation")
        if kind == "missing_interface":
            require(record.exit == CARGO_FAILURE_EXIT and record.compile_errors > 0
                    and record.test_result_lines == 0, "RED compile failure required")
        else:
            require(record.exit in TEST_FAILURE_EXITS, "RED failure exit required")
        for case_id in case_ids:
            case = self.required.get(case_id)
            require(case is not None, f"RED case not in scope: {case_id}")
            require(case.task == task["task"] and case.repository == run.repository,
                    f"RED case outside task: {case_id}")
            require(kind in case.red, f"RED kind not allowed: {case_id}")
            if kind != "missing_interface":
                require(record.status(case.runner, case.key) == "failed",
                        f"RED assertion missing: {case_id}")
                require(not case.database or (record.database == self.fixture
                                              and record.host in LINUX_HOSTS),
                        f"RED database run required: {case_id}")
            for path in case.source_paths:
                before = source.blob(record.source.sha, path)
                require(before is not None and before == source.blob(candidate, path),
                        f"RED and GREEN test source mismatch: {case_id}")
        return run, case_ids

    def check_control(self, entry, run):
        """One private mutation, applied in the run, absent from the candidate."""
        reference = entry.get("patch")
        patch = read_artifact(reference, self.root)
        record = run.record
        require(record.source.control_patch_sha256 == reference["sha256"]
                and record.source.diff_sha256 not in (None, phase6_record.EMPTY_DIFF_SHA256),
                "control patch not applied in run")
        source = self.sources[run.repository]
        require(source.applies(record.source.sha, patch), "control patch does not match source")
        require(not source.applies(self.candidates[run.repository], patch, reverse=True),
                "control mutation is committed")

    # Review and publication.

    def check_review(self):
        if not any(case.runner == "review" for case in self.required.values()):
            return
        review = self.bundle.get("review")
        require(isinstance(review, dict), "review record required")
        try:
            text = read_artifact(review, self.root).decode("utf-8")
        except UnicodeError as error:
            raise EvidenceError("review file must be text") from error
        verdicts = {verdict.lower() for verdict in VERDICT_LINE.findall(text)}
        require(review.get("verdict") == ACCEPTING_VERDICT and verdicts == {ACCEPTING_VERDICT},
                "accepting review verdict required")
        require(all(sha in text for sha in self.candidates.values()),
                "review must name the candidate")
        require(named(review.get("reviewer"))
                and not same_person(review["reviewer"], self.implementer),
                "independent reviewer required")

    def check_publication(self):
        record = self.bundle.get("publication")
        require(isinstance(record, dict) and type(record.get("requested")) is bool,
                "publication state required")
        if not record["requested"]:
            return "not-requested"
        source, candidate = self.sources["backend"], self.candidates["backend"]
        base = record.get("base_sha")
        source.commit(base, "publication base required")
        require(base != candidate and source.is_ancestor(base, candidate),
                "publication base required")
        commits = set(source.git("rev-list", f"{base}..{candidate}").decode().splitlines())
        for field in ("approved_shas", "reviewed_shas"):
            values = record.get(field)
            require(isinstance(values, list) and len(values) == len(set(values))
                    and set(values) == commits, "exact publication ancestry required")
        approver = record.get("approver")
        require(named(approver), "publication approver required")
        require(not same_person(approver, self.implementer),
                "independent publication approver required")
        return "approved"


def validate_evidence(bundle, *, artifact_root, repositories):
    """Return a neutral verdict or raise EvidenceError for incomplete or inconsistent evidence.

    `repositories` maps "backend" (required) and "frontend" (optional) to local
    repository paths.
    """
    try:
        return _Gate(bundle, artifact_root, repositories).validate()
    except EvidenceError:
        raise
    except (KeyError, TypeError, AttributeError, OSError, ValueError) as error:
        raise EvidenceError("malformed evidence record") from error


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--repository", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--frontend-repository", type=Path)
    args = parser.parse_args(argv)
    repositories = {"backend": args.repository, "frontend": args.frontend_repository}
    try:
        verdict = validate_evidence(load_json(args.evidence), artifact_root=args.artifacts,
                                    repositories=repositories)
    except EvidenceError as error:
        print(json.dumps({"status": "refused", "reason": str(error)}))
        return 1
    print(json.dumps({"status": "pass", **verdict}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

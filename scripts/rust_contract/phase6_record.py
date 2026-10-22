"""Turn test-host wrapper logs into ordered stream evidence run records.

A wrapper log starts with the `== verdaxis-<host>-test: <lane> at <sha>` header and
ends with the caller's `CALL_EXIT=<code>` line. Between them, the markers that
`phase6_markers.sh` prints describe the source state and each gate section:

    SOURCE_SHA=<commit>                  starts a new source state
    SOURCE_TREE=<tree>                   optional; the gate compares it with Git
    SOURCE_DIFF_SHA256=<digest>          digest of `git diff --binary HEAD`
    SOURCE_UNTRACKED=<count>             untracked, not ignored files
    CONTROL_PATCH_SHA256=<digest>        one applied control mutation (optional)
    GATE=<NAME> UTC=<time>               opens a gate section
    COMMAND=<text>                       optional; the command of the section
    <NAME>_EXIT=<code> [UTC=<time>]      closes the section with the command's own exit

A section without a finish time on its exit line finishes at the next UTC stamp
(`GATE=... UTC=`, `RUN_END_UTC=` or a bare stamp line). A log without any gate
section is read as one section named CALL: it starts at the first stamp, finishes
at the last stamp and has the CALL_EXIT code.

Each section becomes one run record with the per-test outcomes that cargo (libtest
lines), pytest (`-rA` summary or `-v` lines), Vitest (verbose reporter) and the
journey harness (`[PASS]`/`[FAIL]` lines) print. The adapter reads text only: the
records show that a log is internally consistent and ordered, not which host wrote it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from dataclasses import asdict, dataclass, field, replace
from datetime import datetime
from pathlib import Path

EMPTY_DIFF_SHA256 = hashlib.sha256(b"").hexdigest()
WRAPPER_HOSTS = {"m1": "m1", "staging": "staging", "prod": "prod"}
STAMP_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
STAMP_PATTERN = r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ"

HEADER = re.compile(r"== verdaxis-(m1|staging|prod)-test: (\S+) at ([0-9a-f]{40}) \(host [^)]*\)")
CALL_EXIT = re.compile(r"CALL_EXIT=(-?\d+)")
STAMP_LINE = re.compile(rf"(?:[A-Z][A-Z0-9_]*_UTC=)?({STAMP_PATTERN})")
GATE = re.compile(rf"GATE=([A-Z][A-Z0-9_]*) UTC=({STAMP_PATTERN})")
EXIT = re.compile(rf"([A-Z][A-Z0-9_]*)_EXIT=(-?\d+)(?: UTC=({STAMP_PATTERN}))?")
SOURCE = re.compile(r"(SOURCE_SHA|SOURCE_TREE|SOURCE_DIFF_SHA256|SOURCE_UNTRACKED|"
                    r"CONTROL_PATCH_SHA256)=(\S+)")
BARE_SHA = re.compile(r"[0-9a-f]{40}")
SHA = re.compile(r"[0-9a-f]{40}")
DIGEST = re.compile(r"[0-9a-f]{64}")
COUNT = re.compile(r"\d+")

# The disposable PostgreSQL harness prints the pinned image it starts.
DATABASE = re.compile(r"Starting (\S+@sha256:[0-9a-f]{64}) as verdaxis-analytics-test-\S+")
COMPILE_ERROR = re.compile(r"error\[E\d{4}\]")
TEST_RESULT = "test result: "

CARGO_LINE = re.compile(r"test (\S+) \.\.\. (ok|FAILED|ignored)(?:,.*)?")
PYTEST_SUMMARY = re.compile(r"(PASSED|FAILED|ERROR|SKIPPED|XFAIL|XPASS) (\S+::\S+)(?: - .*)?")
PYTEST_VERBOSE = re.compile(r"(\S+::\S+) (PASSED|FAILED|ERROR|SKIPPED|XFAIL|XPASS)(?: .*)?")
VITEST_LINE = re.compile(r"\s*([✓×↓])\s+(\S.*? > .*?\S)(?:\s+\d+(?:\.\d+)?ms)?")
JOURNEY_LINE = re.compile(r"\s*\[(PASS|FAIL)\] (\S+)(?: .*)?")
PYTEST_PARAMETERS = re.compile(r"\[.*\]\Z")

OUTCOMES = {
    "ok": "passed", "PASSED": "passed", "✓": "passed", "PASS": "passed",
    "FAILED": "failed", "×": "failed", "FAIL": "failed",
    "ERROR": "error",
    "ignored": "skipped", "SKIPPED": "skipped", "XFAIL": "skipped", "XPASS": "skipped",
    "↓": "skipped",
}
# An expected failure or an unexpected pass is not a pass and not an assertion RED.
STATUS_PRECEDENCE = ("failed", "error", "skipped")


class LogError(ValueError):
    """A neutral reason why a wrapper log cannot become run records."""


def require(condition, reason):
    if not condition:
        raise LogError(reason)


def case_status(outcomes):
    """Combine every outcome of one test in one run: any failure wins, then errors, then skips."""
    if not outcomes:
        return "missing"
    for status in STATUS_PRECEDENCE:
        if status in outcomes:
            return status
    return "passed"


@dataclass
class SourceState:
    sha: str
    tree: str | None = None
    diff_sha256: str | None = None
    untracked: int | None = None
    control_patch_sha256: str | None = None

    @property
    def clean(self):
        """True for an unmodified commit, False for a modified tree, None when not recorded."""
        if self.diff_sha256 is None or self.untracked is None:
            return None
        return (self.diff_sha256 == EMPTY_DIFF_SHA256 and self.untracked == 0
                and self.control_patch_sha256 is None)

    def set(self, marker, value):
        if marker == "SOURCE_TREE":
            require(SHA.fullmatch(value) and self.tree is None, "invalid source tree marker")
            self.tree = value
        elif marker == "SOURCE_DIFF_SHA256":
            require(DIGEST.fullmatch(value) and self.diff_sha256 is None,
                    "invalid source diff marker")
            self.diff_sha256 = value
        elif marker == "SOURCE_UNTRACKED":
            require(COUNT.fullmatch(value) and self.untracked is None,
                    "invalid untracked file marker")
            self.untracked = int(value)
        else:
            require(DIGEST.fullmatch(value), "invalid control patch marker")
            require(self.control_patch_sha256 is None, "one control mutation per run")
            self.control_patch_sha256 = value


@dataclass
class Run:
    name: str
    position: int
    host: str
    lane: str
    source: SourceState
    started_at: str
    finished_at: str | None = None
    exit: int | None = None
    command: str | None = None
    database: str | None = None
    compile_errors: int = 0
    test_result_lines: int = 0
    outcomes: dict = field(default_factory=dict)

    def status(self, runner, key):
        return case_status(self.outcomes.get(runner, {}).get(key, []))

    def suite_passed(self, suite):
        """Whether a pytest suite file has at least one passing test in this run."""
        prefix = f"{suite}::"
        return any(key.startswith(prefix) and case_status(outcomes) == "passed"
                   for key, outcomes in self.outcomes.get("pytest", {}).items())

    def read(self, line):
        """Record one output line of the section's commands."""
        if match := DATABASE.fullmatch(line):
            require(self.database in (None, match.group(1)), "database fixture changed in a run")
            self.database = match.group(1)
        elif COMPILE_ERROR.match(line):
            self.compile_errors += 1
        elif line.startswith(TEST_RESULT):
            self.test_result_lines += 1
        elif match := CARGO_LINE.fullmatch(line):
            self.add("cargo", match.group(1), match.group(2))
        elif match := PYTEST_SUMMARY.fullmatch(line):
            self.add("pytest", pytest_key(match.group(2)), match.group(1))
        elif match := PYTEST_VERBOSE.fullmatch(line):
            self.add("pytest", pytest_key(match.group(1)), match.group(2))
        elif match := VITEST_LINE.fullmatch(line):
            self.add("vitest", match.group(2), match.group(1))
        elif match := JOURNEY_LINE.fullmatch(line):
            self.add("journey", match.group(2), match.group(1))

    def add(self, runner, key, marker):
        self.outcomes.setdefault(runner, {}).setdefault(key, []).append(OUTCOMES[marker])

    def as_record(self):
        record = asdict(self)
        record["source"]["clean"] = self.source.clean
        return record


def pytest_key(node_id):
    """Use `path::name` for every parameter set of one pytest function."""
    return PYTEST_PARAMETERS.sub("", node_id)


class _Reader:
    def __init__(self, host, lane):
        self.host = host
        self.lane = lane
        self.last_stamp = None

    def stamp(self, text):
        try:
            parsed = datetime.strptime(text, STAMP_FORMAT)
        except ValueError as error:
            raise LogError("invalid UTC stamp") from error
        require(self.last_stamp is None or parsed >= self.last_stamp, "log stamps out of order")
        self.last_stamp = parsed
        return text

    def new_run(self, name, position, state, started_at):
        return Run(name=name, position=position, host=self.host, lane=self.lane,
                   source=replace(state), started_at=started_at)

    def sections(self, body):
        runs, state, current, waiting = [], None, None, None
        for line in body:
            if match := GATE.fullmatch(line):
                name, started_at = match.groups()
                require(current is None, "gate section not closed")
                require(state is not None, "source identity required")
                require(all(run.name != name for run in runs), "duplicate gate name")
                started_at = self.stamp(started_at)
                if waiting is not None:
                    waiting.finished_at, waiting = started_at, None
                current = self.new_run(name, len(runs) + 1, state, started_at)
                runs.append(current)
            elif (match := EXIT.fullmatch(line)) and current is not None \
                    and match.group(1) == current.name:
                current.exit = int(match.group(2))
                if match.group(3):
                    current.finished_at = self.stamp(match.group(3))
                else:
                    waiting = current
                current = None
            elif match := STAMP_LINE.fullmatch(line):
                stamped = self.stamp(match.group(1))
                if waiting is not None:
                    waiting.finished_at, waiting = stamped, None
            elif match := SOURCE.fullmatch(line):
                require(current is None, "source changed inside a gate section")
                marker, value = match.groups()
                if marker == "SOURCE_SHA":
                    require(SHA.fullmatch(value), "invalid source marker")
                    state = SourceState(sha=value)
                else:
                    require(state is not None, "source identity required")
                    state.set(marker, value)
            elif current is not None:
                if line.startswith("COMMAND=") and current.command is None:
                    current.command = line.removeprefix("COMMAND=")
                else:
                    current.read(line)
        require(current is None, "gate section not closed")
        require(waiting is None, "gate finish time missing")
        return runs

    def whole_call(self, body, exit_code):
        state, stamps, output = None, [], []
        for line in body:
            if match := STAMP_LINE.fullmatch(line):
                stamps.append(self.stamp(match.group(1)))
                continue
            marker = SOURCE.fullmatch(line)
            # Older single-command logs print the commit as a bare line.
            if state is None and (BARE_SHA.fullmatch(line)
                                  or marker and marker.group(1) == "SOURCE_SHA"):
                sha = marker.group(2) if marker else line
                require(SHA.fullmatch(sha), "invalid source marker")
                state = SourceState(sha=sha)
            elif marker:
                require(state is not None, "source identity required")
                require(marker.group(1) != "SOURCE_SHA", "one source identity required")
                state.set(*marker.groups())
            elif state is not None:
                output.append(line)
        require(state is not None, "source identity required")
        require(len(stamps) >= 2, "start and finish stamps required")
        run = self.new_run("CALL", 1, state, stamps[0])
        run.finished_at, run.exit = stamps[-1], exit_code
        for line in output:
            run.read(line)
        return run


def parse_log(data):
    """Return the ordered run records of one wrapper log, or raise LogError."""
    lines = data.decode("utf-8", errors="replace").splitlines()
    while lines and not lines[-1].strip():
        lines.pop()
    require(lines and CALL_EXIT.fullmatch(lines[-1]), "terminal exit marker required")
    headers = [index for index, line in enumerate(lines) if HEADER.fullmatch(line)]
    require(len(headers) == 1, "one wrapper header required")
    host, lane, _ = HEADER.fullmatch(lines[headers[0]]).groups()
    body = lines[headers[0] + 1:-1]
    reader = _Reader(WRAPPER_HOSTS[host], lane)
    if any(GATE.fullmatch(line) for line in body):
        return reader.sections(body)
    return [reader.whole_call(body, int(CALL_EXIT.fullmatch(lines[-1]).group(1)))]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("logs", nargs="+", type=Path)
    args = parser.parse_args(argv)
    documents = []
    for path in args.logs:
        try:
            data = path.read_bytes()
            runs = parse_log(data)
        except (OSError, LogError) as error:
            reason = str(error) if isinstance(error, LogError) else "log unavailable"
            print(json.dumps({"status": "refused", "log": str(path), "reason": reason}))
            return 1
        documents.append({"log": str(path), "sha256": hashlib.sha256(data).hexdigest(),
                          "runs": [run.as_record() for run in runs]})
    print(json.dumps(documents, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

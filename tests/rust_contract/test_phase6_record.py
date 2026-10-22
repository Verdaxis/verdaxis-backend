"""Wrapper logs become ordered run records for the stream evidence gate."""
from __future__ import annotations

import hashlib
import importlib
import json
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = "a" * 40
WRAPPER = "b" * 40
TREE = "c" * 40
EMPTY_DIFF = hashlib.sha256(b"").hexdigest()
IMAGE = "postgis/postgis:17-3.6-alpine@sha256:" + "4" * 64
HEADER = f"== verdaxis-staging-test: rust-p6-gate at {WRAPPER} (host fixture)"


def adapter():
    module_path = ROOT / "scripts/rust_contract/phase6_record.py"
    assert module_path.is_file(), "wrapper log adapter is not implemented"
    return importlib.import_module("scripts.rust_contract.phase6_record")


def parse(*lines):
    return adapter().parse_log(("\n".join(lines) + "\n").encode())


def refused(reason, *lines):
    module = adapter()
    with pytest.raises(module.LogError) as caught:
        parse(*lines)
    assert str(caught.value) == reason


def section(name="TESTS", *body, exit_code=0):
    return [f"GATE={name} UTC=2026-10-22T10:00:00Z", *body,
            f"{name}_EXIT={exit_code} UTC=2026-10-22T10:01:00Z"]


def test_record_reads_gate_sections():
    unit, database = parse(
        "verdaxis-staging-test: poll failed (connection lost?); the remote run continues",
        HEADER,
        "RUN_START_UTC=2026-10-22T10:00:00Z",
        f"SOURCE_SHA={SOURCE}", f"SOURCE_TREE={TREE}", f"SOURCE_DIFF_SHA256={EMPTY_DIFF}",
        "SOURCE_UNTRACKED=0",
        "GATE=UNIT UTC=2026-10-22T10:00:01Z",
        "COMMAND=cargo test --test streams_bus",
        "     Running tests/streams_bus.rs (rs/target/debug/deps/streams_bus-0)",
        "test bounded_queue ... ok",
        "test overflow_resets ... FAILED",
        "test needs_database ... ignored, requires the disposable PostgreSQL harness",
        "test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out",
        "UNIT_EXIT=101",
        "GATE=POSTGRES UTC=2026-10-22T10:02:00Z",
        f"Starting {IMAGE} as verdaxis-analytics-test-1-2",
        "PASSED tests/postgres/test_phase6_x.py::test_component",
        "POSTGRES_EXIT=0",
        "RUN_END_UTC=2026-10-22T10:05:00Z",
        "CALL_EXIT=1",
    )
    assert (unit.name, unit.position, unit.host, unit.lane) == ("UNIT", 1, "staging",
                                                               "rust-p6-gate")
    assert (unit.started_at, unit.finished_at, unit.exit) == (
        "2026-10-22T10:00:01Z", "2026-10-22T10:02:00Z", 101)
    assert unit.command == "cargo test --test streams_bus"
    assert (unit.source.sha, unit.source.tree, unit.source.clean) == (SOURCE, TREE, True)
    assert unit.status("cargo", "bounded_queue") == "passed"
    assert unit.status("cargo", "overflow_resets") == "failed"
    assert unit.status("cargo", "needs_database") == "skipped"
    assert unit.status("cargo", "absent") == "missing"
    assert (unit.test_result_lines, unit.compile_errors, unit.database) == (1, 0, None)
    assert (database.position, database.finished_at, database.exit) == (
        2, "2026-10-22T10:05:00Z", 0)
    assert database.database == IMAGE
    assert database.suite_passed("tests/postgres/test_phase6_x.py")
    assert not database.suite_passed("tests/postgres/test_phase6_y.py")


def test_record_reads_single_command_logs():
    (run,) = parse(
        f"== verdaxis-m1-test: rust-p6-gate at {WRAPPER} (host fixture)",
        f"== tree: 0 changed tracked file(s) and 0 untracked file(s) on top of {WRAPPER}",
        "2026-10-22T10:56:43Z",
        WRAPPER,
        "error[E0432]: unresolved import `verdaxis_read::streams::bus`",
        "FAILED tests/rust_contract/test_x.py::test_one - AssertionError: absent",
        "1 failed in 1.00s",
        "2026-10-22T10:56:53Z",
        "CALL_EXIT=1",
    )
    assert (run.name, run.host, run.exit, run.compile_errors) == ("CALL", "m1", 1, 1)
    assert (run.started_at, run.finished_at) == ("2026-10-22T10:56:43Z",
                                                 "2026-10-22T10:56:53Z")
    assert (run.source.sha, run.source.clean) == (WRAPPER, None)
    assert run.status("pytest", "tests/rust_contract/test_x.py::test_one") == "failed"


def test_record_reads_runner_lines():
    (run,) = parse(HEADER, f"SOURCE_SHA={SOURCE}", *section(
        "TESTS",
        "tests/a.py::test_case[one] PASSED                                [ 50%]",
        "tests/a.py::test_case[two] PASSED                                [100%]",
        "PASSED tests/a.py::test_case[one]",
        "ERROR tests/a.py::test_setup - RuntimeError: fixture",
        "tests/a.py::test_skip SKIPPED (not here)                          [100%]",
        "XFAIL tests/a.py::test_expected - reason",
        " ✓ src/tests/use-sse.test.tsx > stream > reconnects 12ms",
        " × src/tests/use-sse.test.tsx > stream > resets 3ms",
        " ↓ src/tests/use-sse.test.tsx > stream > later",
        " ✓ src/tests/use-sse.test.tsx (3 tests) 20ms",
        "  [PASS] J61 public refresh",
        "  [FAIL] J62 private refresh: timeout",
        exit_code=1), "CALL_EXIT=1")
    assert run.status("pytest", "tests/a.py::test_case") == "passed"
    assert run.status("pytest", "tests/a.py::test_setup") == "error"
    assert run.status("pytest", "tests/a.py::test_skip") == "skipped"
    assert run.status("pytest", "tests/a.py::test_expected") == "skipped"
    assert run.outcomes["vitest"] == {
        "src/tests/use-sse.test.tsx > stream > reconnects": ["passed"],
        "src/tests/use-sse.test.tsx > stream > resets": ["failed"],
        "src/tests/use-sse.test.tsx > stream > later": ["skipped"],
    }
    assert run.outcomes["journey"] == {"J61": ["passed"], "J62": ["failed"]}
    assert run.source.clean is None


def test_record_status_precedence():
    status = adapter().case_status
    assert status([]) == "missing"
    assert status(["passed", "passed"]) == "passed"
    assert status(["passed", "skipped"]) == "skipped"
    assert status(["skipped", "error"]) == "error"
    assert status(["error", "passed", "failed"]) == "failed"


def test_record_requires_terminal_exit():
    refused("terminal exit marker required", HEADER, f"SOURCE_SHA={SOURCE}", *section())


def test_record_requires_one_header():
    refused("one wrapper header required", HEADER, HEADER, f"SOURCE_SHA={SOURCE}",
            *section(), "CALL_EXIT=0")
    refused("one wrapper header required", f"SOURCE_SHA={SOURCE}", *section(), "CALL_EXIT=0")


def test_record_requires_closed_sections():
    refused("gate section not closed", HEADER, f"SOURCE_SHA={SOURCE}",
            "GATE=TESTS UTC=2026-10-22T10:00:00Z", "CALL_EXIT=0")


def test_record_requires_finish_time():
    refused("gate finish time missing", HEADER, f"SOURCE_SHA={SOURCE}",
            "GATE=TESTS UTC=2026-10-22T10:00:00Z", "TESTS_EXIT=0", "CALL_EXIT=0")


def test_record_requires_ordered_stamps():
    refused("log stamps out of order", HEADER, f"SOURCE_SHA={SOURCE}",
            "GATE=TESTS UTC=2026-10-22T10:05:00Z", "TESTS_EXIT=0 UTC=2026-10-22T10:00:00Z",
            "CALL_EXIT=0")


def test_record_requires_source_identity():
    refused("source identity required", HEADER, *section(), "CALL_EXIT=0")


def test_record_refuses_source_change_inside_section():
    refused("source changed inside a gate section", HEADER, f"SOURCE_SHA={SOURCE}",
            "GATE=TESTS UTC=2026-10-22T10:00:00Z", f"SOURCE_SHA={WRAPPER}",
            "TESTS_EXIT=0 UTC=2026-10-22T10:01:00Z", "CALL_EXIT=0")


def test_record_refuses_two_control_mutations():
    refused("one control mutation per run", HEADER, f"SOURCE_SHA={SOURCE}",
            "CONTROL_PATCH_SHA256=" + "1" * 64, "CONTROL_PATCH_SHA256=" + "2" * 64,
            *section(), "CALL_EXIT=0")


def test_record_refuses_duplicate_gate_names():
    refused("duplicate gate name", HEADER, f"SOURCE_SHA={SOURCE}", *section(), *section(),
            "CALL_EXIT=0")


def test_record_reads_marker_helper_output(tmp_path):
    helper = ROOT / "scripts/rust_contract/phase6_markers.sh"
    assert helper.is_file(), "marker helper is not implemented"
    repository = tmp_path / "repository"
    repository.mkdir()

    def git(*args):
        return subprocess.run(["git", "-C", str(repository), *args], check=True,
                              capture_output=True, text=True).stdout.strip()

    git("init", "-q")
    (repository / "value.txt").write_text("complete\n")
    git("add", "value.txt")
    git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
        "-c", "commit.gpgsign=false", "commit", "-qm", "start")
    patch = tmp_path / "control.patch"
    patch.write_text("diff --git a/value.txt b/value.txt\n--- a/value.txt\n"
                     "+++ b/value.txt\n@@ -1 +1 @@\n-complete\n+mutated\n")
    script = (f'. "{helper}"\nphase6_source\nphase6_gate PASSING true\n'
              f'phase6_gate FAILING false\nphase6_control "{patch}"\n'
              "phase6_gate CONTROL false\nexit 0\n")
    output = subprocess.run(["bash", "-c", script], cwd=repository, capture_output=True,
                            text=True, check=True).stdout
    sha = git("rev-parse", "HEAD")
    log = f"== verdaxis-m1-test: fixture at {sha} (host fixture)\n{output}CALL_EXIT=0\n"
    passing, failing, control = adapter().parse_log(log.encode())
    assert (passing.exit, failing.exit, control.exit) == (0, 1, 1)
    assert passing.command == "true"
    assert (passing.source.sha, passing.source.tree) == (sha, git("rev-parse", "HEAD^{tree}"))
    assert passing.source.clean is True and failing.source.clean is True
    assert control.source.clean is False
    assert control.source.control_patch_sha256 == hashlib.sha256(patch.read_bytes()).hexdigest()
    assert control.source.diff_sha256 != EMPTY_DIFF


def test_record_cli_prints_records(tmp_path, capsys):
    log = tmp_path / "run.log"
    log.write_text("\n".join([HEADER, f"SOURCE_SHA={SOURCE}",
                              *section("TESTS", "test bounded_queue ... ok"),
                              "CALL_EXIT=0"]) + "\n")
    assert adapter().main([str(log)]) == 0
    (document,) = json.loads(capsys.readouterr().out)
    assert document["sha256"] == hashlib.sha256(log.read_bytes()).hexdigest()
    (run,) = document["runs"]
    assert (run["name"], run["exit"], run["source"]["clean"]) == ("TESTS", 0, None)
    assert run["outcomes"] == {"cargo": {"bounded_queue": ["passed"]}}


def test_record_cli_reports_refusal(tmp_path, capsys):
    log = tmp_path / "run.log"
    log.write_text(f"{HEADER}\n")
    assert adapter().main([str(log)]) == 1
    assert json.loads(capsys.readouterr().out) == {
        "status": "refused", "log": str(log), "reason": "terminal exit marker required"}

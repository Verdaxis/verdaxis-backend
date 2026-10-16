"""Fail-closed evidence checks for the existing disposable runner."""
from pathlib import Path

import pytest

from tests.phase0 import runner


def report(tmp_path, cases):
    path = tmp_path / "pytest.xml"
    path.write_text('<testsuites><testsuite>' + cases + '</testsuite></testsuites>')
    return path


@pytest.mark.parametrize("cases,required", [
    ('', {"tests.phase0.test_inventory"}),
    ('<testcase classname="tests.phase0.test_copy" name="test_copy"/>',
     {"tests.phase0.test_inventory"}),
    ('<testcase classname="tests.phase0.test_inventory" name="test_inventory"><skipped/></testcase>',
     {"tests.phase0.test_inventory"}),
    ('<testcase classname="tests.phase0.test_inventory" name="test_inventory"><failure/></testcase>',
     {"tests.phase0.test_inventory"}),
])
def test_runner_refuses_incomplete_evidence(tmp_path, cases, required):
    with pytest.raises(ValueError):
        runner.check_test_report(report(tmp_path, cases), required, set())


def test_runner_requires_review_focus_case(tmp_path):
    path = report(tmp_path, '<testcase classname="tests.phase0.test_copy" name="test_other"/>')
    with pytest.raises(ValueError, match="required tests"):
        runner.check_test_report(path, {"tests.phase0.test_copy"}, {"test_copy_cycles_and_next_sequence_values"})


def test_runner_records_counts_and_parameterized_focus(tmp_path):
    path = report(tmp_path, '<testcase classname="tests.phase0.test_copy" name="test_copy_cycles_and_next_sequence_values[empty]"/>')
    result = runner.check_test_report(path, {"tests.phase0.test_copy"}, {"test_copy_cycles_and_next_sequence_values"})
    assert result["passed"] == 1
    assert result["skipped"] == result["failed"] == 0


def test_full_runner_requires_every_phase0_suite():
    paths = runner.default_test_paths()
    expected = set(Path("tests/phase0").glob("test_*.py"))
    expected.update(Path("tests/postgres").glob("test_phase0_*.py"))
    assert set(map(Path, paths)) == expected


def test_runner_required_focus_covers_local_boundaries():
    assert runner.REQUIRED_TESTS == {
        "test_historical_snapshot_import_restores_exact_guards",
        "test_copy_cycles_and_next_sequence_values",
        "test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked",
        "test_timezone_reconnect_and_role_override",
        "test_bootstrap_uses_only_checked_connection",
        "test_outbox_drain_before_final_snapshot",
    }

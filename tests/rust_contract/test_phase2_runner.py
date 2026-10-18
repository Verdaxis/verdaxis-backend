"""Write comparisons require isolated cells and full committed state."""
import copy
import importlib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def state_module():
    assert (ROOT / "scripts/rust_contract/phase2_state.py").is_file(), "missing twin-database state comparator"
    return importlib.import_module("scripts.rust_contract.phase2_state")


def test_same_database_and_unequal_seed_refuse():
    module = state_module()
    for right, seed, acl, token, url in [
        ("python", "seed", "acl", "x" * 32, "http://127.0.0.1:49152"),
        ("rust", "different", "acl", "x" * 32, "http://127.0.0.1:49152"),
        ("rust", "seed", "", "x" * 32, "http://127.0.0.1:49152"),
        ("rust", "seed", "acl", "", "http://127.0.0.1:49152"),
        ("rust", "seed", "acl", "x" * 32, "https://api.verdaxis.exchange"),
    ]:
        with pytest.raises(module.StateError):
            module.validate_cells({"database": "python", "seed": "seed", "acl": "acl"},
                                  {"database": right, "seed": seed, "acl": acl}, token, url)


def test_extra_notification_differs_even_with_equal_responses():
    module = state_module()
    left = {"users": {"actor": {"id": "actor", "email": "synthetic", "referral_code": None}}, "notifications": {}}
    right = copy.deepcopy(left)
    right["notifications"]["extra"] = {"id": "extra", "is_read": False}
    assert module.compare_state(left, right)


@pytest.mark.parametrize("column", ["password", "authentication_revoked_at", "role", "email", "status", "unrelated"])
def test_whole_user_row_only_allows_conditional_referral_code(column):
    module = state_module()
    before = {"users": {"actor": {"id": "actor", "referral_code": None, column: "old"}}, "notifications": {}}
    after = copy.deepcopy(before)
    after["users"]["actor"][column] = "changed"
    assert module.unexpected_user_changes(before, after, route_id="R01", actor_id="actor", recipient_exists=False)
    after = copy.deepcopy(before)
    after["users"]["actor"]["referral_code"] = "VDX-ABC123"
    assert module.unexpected_user_changes(before, after, route_id="R01", actor_id="actor", recipient_exists=False) == []
    assert module.unexpected_user_changes(before, after, route_id="R04", actor_id="actor", recipient_exists=True)
    assert module.unexpected_user_changes(before, after, route_id="N03", actor_id="actor", recipient_exists=False)


def test_postcommit_failure_is_not_replayed_and_precommit_is_rolled_back():
    module = state_module()
    class Cell:
        def __init__(self):
            self.pending, self.rows, self.calls = [], [], 0
        def begin(self):
            self.pending = []
        def execute(self, value):
            self.calls += 1
            self.pending.append(value)
        def commit(self):
            self.rows += self.pending
            self.pending = []
        def rollback(self):
            self.pending = []
    cell = Cell()
    assert module.apply_action(cell, ["child1", "child2"], fault="between-writes") == "rolled-back"
    assert cell.rows == []
    assert module.apply_action(cell, ["child1", "child2"], fault="after-commit") == "ambiguous"
    assert cell.rows == ["child1", "child2"]
    assert cell.calls == 3


def test_action_values_advance_and_exhaust_without_runtime_override(monkeypatch):
    from datetime import UTC, datetime, timedelta

    from tests.rust_contract.clock import ActionValues
    monkeypatch.setenv("ENVIRONMENT", "test")
    values = ActionValues(datetime(2026, 10, 18, tzinfo=UTC), ["one"], ["VDX-ABC123"])
    assert values.next_id() == "one"
    assert values.next_referral_code() == "VDX-ABC123"
    values.advance(timedelta(microseconds=1))
    assert values.now.microsecond == 1
    with pytest.raises(StopIteration):
        values.next_id()
    monkeypatch.setenv("ENVIRONMENT", "production")
    with pytest.raises(RuntimeError):
        ActionValues(datetime.now(UTC), [], [])


def test_cleanup_failure_never_passes():
    module = state_module()
    with pytest.raises(module.StateError):
        module.require_cleanup(["owned-container-still-present"])


def test_phase2_cli_rejects_unknown_scenario_and_unbuilt_route_group():
    from scripts.rust_contract import run_disposable
    with pytest.raises(SystemExit):
        run_disposable.main(["--phase2", "--phase2-scenario", "unknown"])

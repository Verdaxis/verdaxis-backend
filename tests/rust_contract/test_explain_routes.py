"""Plan capture keeps statement binds and complete JSON documents."""

import json
from pathlib import Path

from scripts.rust_contract import explain_routes


def test_mcp_old_shape_uses_only_the_shared_order_binds():
    policy = json.loads(Path(explain_routes.POLICY_PATH).read_text())
    old = explain_routes.bind_literals(policy, "mcp_slice_before")
    keyed = explain_routes.bind_literals(policy, "mcp_slice")
    assert len(old) == 13
    assert len(keyed) == 14
    assert keyed[:13] == old
    example = Path("rs/read/examples/explain_statements.rs").read_text()
    assert '"mcp_slice_before"' in example


def test_plan_json_is_copied_as_a_file(monkeypatch):
    plan = {"Plan": {"Node Type": "Result", "detail": "x" * 300_000}}
    calls = []

    def docker(*args, **kwargs):
        calls.append(args)
        assert args[:2] == ("cp", "disposable-plan-test:/tmp/plan.json")
        Path(args[2]).write_text(json.dumps([plan]))
        return ""

    monkeypatch.setattr(explain_routes.runner, "docker", docker)
    capture = object.__new__(explain_routes.PlanCapture)
    capture.container = "disposable-plan-test"
    scripts = []
    monkeypatch.setattr(capture, "sql", scripts.append)
    assert capture.explain("large-plan", "SELECT 1", []) == plan
    assert len(calls) == 1
    assert "TIMING OFF" in scripts[0]
    assert "\\o /tmp/plan.json" in scripts[0]

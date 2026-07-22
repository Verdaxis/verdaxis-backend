from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/status_state.py"


def load_module():
    spec = importlib.util.spec_from_file_location("status_state_contract", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def status(category: str, checks: list[tuple[str, str]]) -> dict:
    return {
        "schema_version": 1,
        "generated_at": "2026-07-20T00:00:00Z",
        "category": category,
        "checks": [
            {
                "name": name,
                "category": check_category,
                "code": "ok" if check_category == "healthy" else "degraded",
                "duration_ms": 1,
                "last_success_at": "2026-07-20T00:00:00Z",
            }
            for name, check_category in checks
        ],
    }


def test_status_requires_at_least_one_check():
    state = load_module()

    assert not state.valid_monitor_status(status("failure", []))


@pytest.mark.parametrize(
    ("top_category", "checks"),
    [
        ("healthy", [("disk", "warning")]),
        ("warning", [("disk", "failure")]),
        ("failure", [("disk", "healthy")]),
        ("warning", [("disk", "healthy"), ("runtime", "healthy")]),
    ],
)
def test_status_top_level_category_must_equal_deterministic_worst_child(
    top_category, checks
):
    state = load_module()

    assert not state.valid_monitor_status(status(top_category, checks))


def test_status_accepts_the_exact_worst_child_category():
    state = load_module()

    assert state.valid_monitor_status(
        status(
            "failure",
            [("runtime", "healthy"), ("disk", "warning"), ("backup", "failure")],
        )
    )


def test_status_loader_rejects_duplicate_json_keys(tmp_path):
    state = load_module()
    path = tmp_path / "status.json"
    path.write_text(
        '{"schema_version":1,"schema_version":1,'
        '"generated_at":"2026-07-20T00:00:00Z","category":"healthy",'
        '"checks":[{"name":"runtime","category":"healthy","code":"ok",'
        '"duration_ms":1,"last_success_at":"2026-07-20T00:00:00Z"}]}'
    )

    loaded = state.load_json(path)

    assert loaded.exists
    assert loaded.malformed
    assert loaded.value is None

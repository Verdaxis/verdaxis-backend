"""Fail-closed contract tests for the demo activity timer CLI."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/run_demo_activity.py"


def load_module():
    spec = importlib.util.spec_from_file_location("run_demo_activity", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize(
    "result",
    [
        {"created_orders": 2, "created_trades": 1},
        {
            "created_orders": 0,
            "created_trades": 0,
            "reason": "already generated for tick",
        },
    ],
)
def test_validate_activity_result_accepts_created_or_idempotent_tick(result):
    load_module()._validate_activity_result(result)


@pytest.mark.parametrize(
    "result",
    [
        {
            "created_orders": 0,
            "created_trades": 0,
            "reason": "missing product or delivery point",
        },
        {"created_orders": 0, "created_trades": 0, "reason": "unknown"},
        {"created_orders": 2, "created_trades": 0},
        {"created_orders": True, "created_trades": True},
        {},
    ],
)
def test_validate_activity_result_rejects_all_other_results(result):
    with pytest.raises(RuntimeError, match="unexpected result"):
        load_module()._validate_activity_result(result)


@pytest.mark.asyncio
async def test_main_rejects_invalid_result_before_final_commit(monkeypatch):
    module = load_module()
    db = MagicMock()
    db.commit = AsyncMock()

    class SessionContext:
        async def __aenter__(self):
            return db

        async def __aexit__(self, exc_type, exc, traceback):
            return False

    monkeypatch.setattr(module, "AsyncSessionLocal", SessionContext)
    monkeypatch.setattr(module, "ensure_demo_activity_organizations", AsyncMock())
    monkeypatch.setattr(
        module,
        "ensure_demo_market_coverage",
        AsyncMock(return_value={}),
    )
    monkeypatch.setattr(module, "prune_demo_activity", AsyncMock())
    monkeypatch.setattr(
        module,
        "generate_demo_market_activity",
        AsyncMock(
            return_value={
                "created_orders": 0,
                "created_trades": 0,
                "reason": "missing product or delivery point",
            }
        ),
    )

    with pytest.raises(RuntimeError, match="unexpected result"):
        await module.main()

    assert db.commit.await_count == 3

"""Drift guard for the committed OpenAPI contract.

`openapi.json` at the repository root is the API contract that the frontend
vendors by SHA to generate its API types. This suite fails when the committed
file differs from a fresh export, so a route or schema change must regenerate the
file in the same commit instead of silently changing the published contract.
"""

from __future__ import annotations

import difflib

from scripts import export_openapi

REGENERATE_COMMAND = (
    "ENVIRONMENT=test RELEASE_SHA=test "
    "JWT_SECRET=test-secret-key-for-testing-minimum-32-chars "
    "DATABASE_URL=sqlite+aiosqlite:///:memory: "
    "venv/bin/python scripts/export_openapi.py"
)

_MAX_DIFF_LINES = 40


def _drift_report(committed: str, fresh: str) -> str:
    diff = difflib.unified_diff(
        committed.splitlines(),
        fresh.splitlines(),
        fromfile=f"{export_openapi.OUTPUT_PATH.name} (committed)",
        tofile=f"{export_openapi.OUTPUT_PATH.name} (fresh export)",
        lineterm="",
    )
    shown = list(diff)[:_MAX_DIFF_LINES]
    if len(shown) == _MAX_DIFF_LINES:
        shown.append("... (diff truncated)")
    return (
        f"{export_openapi.OUTPUT_PATH.name} is out of date. Regenerate it and "
        "commit the result:\n\n"
        f"    {REGENERATE_COMMAND}\n\n"
        + "\n".join(shown)
    )


def test_committed_openapi_matches_a_fresh_export():
    if not export_openapi.OUTPUT_PATH.exists():
        raise AssertionError(
            f"{export_openapi.OUTPUT_PATH.name} is missing from the repository "
            f"root. Generate it with:\n\n    {REGENERATE_COMMAND}"
        )
    committed = export_openapi.OUTPUT_PATH.read_text(encoding="utf-8")
    fresh = export_openapi.render(export_openapi.build_document())
    if committed != fresh:
        raise AssertionError(_drift_report(committed, fresh))

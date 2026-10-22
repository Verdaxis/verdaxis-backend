"""Dispatch component checks on the role-separated disposable database."""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def test_committed_dispatch_visibility(analytics_pg_url):
    app_url = os.environ["DATABASE_URL"]
    assert app_url.startswith("postgresql+asyncpg://")
    result = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests", "--test",
            "streams_dispatch", "postgres::committed_visibility_and_rollback",
            "--", "--exact", "--ignored", "--test-threads=1",
        ],
        cwd=ROOT,
        env={
            **os.environ,
            "SQLX_OFFLINE": "true",
            "RELEASE_SHA": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
            ).strip(),
            "STREAM_DISPATCH_APP_URL": app_url.replace(
                "postgresql+asyncpg://", "postgresql://", 1
            ),
        },
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    print(result.stdout)
    print(result.stderr[-4000:])
    assert result.returncode == 0
    assert re.search(r"test postgres::committed_visibility_and_rollback \.\.\. ok", result.stdout)
    assert "1 passed; 0 failed; 0 ignored" in result.stdout


@pytest.mark.skip(reason="Task 2 store/migration integration required; not retention acceptance")
def test_retention_snapshot_resets_existing_subscribers(analytics_pg_url):
    # Integration owner supplies the real store after Task 2 merges. The case
    # must initialize the hub, register both public channels and an organization,
    # commit a permitted migrator DELETE, then scan the advanced floor. Expect
    # one cursor-free replay_retention reset, no queued business row, and a fresh
    # baseline; rolling back the DELETE must leave the old cursor valid.
    pytest.fail("replace this pending case with the Task 2 store/trigger fixture")

"""Dispatch component checks on the role-separated disposable database."""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest_asyncio
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.retention_test_support import clear_retention


@pytest_asyncio.fixture(autouse=True)
async def isolated_dispatch_history(analytics_pg_url):
    # Other stream suites intentionally leave high floors and synthetic IDs.
    # Reset only this disposable fixture before each global dispatch proof.
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.begin() as connection:
            await clear_retention(connection)
        yield
    finally:
        await engine.dispose()


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


def test_retention_snapshot_resets_existing_subscribers(analytics_pg_url):
    result = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
         "-p", "verdaxis-read", "--features", "contract-tests", "--test",
         "streams_dispatch", "postgres::retention_snapshot_resets_existing_subscribers",
         "--", "--exact", "--ignored", "--test-threads=1"],
        cwd=ROOT,
        env={**os.environ, "SQLX_OFFLINE": "true",
             "STREAM_DISPATCH_APP_URL": os.environ["DATABASE_URL"].replace(
                 "postgresql+asyncpg://", "postgresql://", 1),
             "STREAM_DISPATCH_OWNER_URL": analytics_pg_url.replace(
                 "postgresql+asyncpg://", "postgresql://", 1)},
        capture_output=True, text=True, timeout=1800, check=False,
    )
    print(result.stdout)
    print(result.stderr)
    assert result.returncode == 0
    assert "test postgres::retention_snapshot_resets_existing_subscribers ... ok" in result.stdout
    assert "1 passed; 0 failed; 0 ignored" in result.stdout

"""Run stream lifecycle cases on the role-separated disposable PostgreSQL fixture."""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from tests.postgres.test_phase6_http import _require_app_role, _sqlx_url

ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.asyncio
async def test_stream_runtime_on_disposable_database(analytics_pg_url):
    app_url = os.environ["DATABASE_URL"]
    await _require_app_role(app_url)
    result = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
         "-p", "verdaxis-read", "--features", "contract-tests", "--test",
         "streams_runtime", "--", "--ignored", "--test-threads=1", "--nocapture"],
        cwd=ROOT,
        env={**os.environ, "SQLX_OFFLINE": "true",
             "RELEASE_SHA": subprocess.check_output(
                 ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
             ).strip(),
             "STREAM_RUNTIME_APP_URL": _sqlx_url(app_url),
             "STREAM_RUNTIME_OWNER_URL": _sqlx_url(analytics_pg_url),
             "STREAM_RUNTIME_ADMIN_URL": _sqlx_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])},
        capture_output=True, text=True, timeout=1800, check=False,
    )
    output = result.stdout + result.stderr
    assert result.returncode == 0, output
    assert "test result: ok. 2 passed; 0 failed; 0 ignored" in output, output

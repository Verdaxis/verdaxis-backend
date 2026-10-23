"""Login component checks on the disposable migrated database."""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

from tests.postgres.conftest import analytics_pg_url  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
CASES = (
    "login_success_returns_access_only_and_sets_both_cookies",
    "login_credential_refusal_vectors",
    "login_account_admission_vectors",
    "login_writes_one_family_per_device",
    "login_second_account_same_device_revokes_first",
    "login_session_failure_rolls_back_and_is_retryable",
)


def test_phase3_login(analytics_pg_url):
    environment = {
        **os.environ,
        "AUTH_LOGIN_OWNER_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgresql://"),
        "AUTH_LOGIN_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgresql://"),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    }
    run = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--features", "contract-tests", "--test", "auth_login_postgres", "--", "--include-ignored", "--test-threads=1"],
        cwd=ROOT, env=environment, capture_output=True, text=True, timeout=1800,
    )
    assert run.returncode == 0, run.stdout[-12000:] + run.stderr[-12000:]
    for name in CASES:
        assert f"test {name} ... ok" in run.stdout, run.stdout

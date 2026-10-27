"""Refresh component checks on disposable PostgreSQL."""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

from tests.postgres.conftest import analytics_pg_url  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
CASES = (
    "refresh_rotates_and_keeps_family",
    "refresh_duplicate_within_grace_is_409",
    "refresh_replay_after_grace_revokes_family",
    "refresh_binding_and_account_vectors",
    "refresh_cutoff_order_password_then_revocation",
    "refresh_issuance_range_vectors",
    "concurrent_refresh_same_token_yields_one_successor",
)

def test_phase3_refresh(analytics_pg_url):
    env = {
        **os.environ,
        "AUTH_REFRESH_OWNER_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgresql://"),
        "AUTH_REFRESH_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgresql://"),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    }
    run = subprocess.run(["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--features", "contract-tests", "--test", "auth_refresh_postgres", "--", "--include-ignored", "--test-threads=1"], cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800)
    assert run.returncode == 0, run.stdout[-16000:] + run.stderr[-16000:]
    for name in CASES:
        assert f"test {name} ... ok" in run.stdout, run.stdout

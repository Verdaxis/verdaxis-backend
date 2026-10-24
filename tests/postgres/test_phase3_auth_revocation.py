"""Run the Rust cutoff tests with the migrated disposable runtime ACLs."""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

from tests.postgres.test_phase3_auth_store import _libpq

ROOT = Path(__file__).resolve().parents[2]
RUST_TESTS = (
    "revocation_cutoff_is_monotonic_under_concurrency",
    "equal_or_earlier_time_advances_cutoff_by_one_microsecond",
    "caller_rollback_preserves_cutoff_and_sessions",
    "user_without_refresh_sessions_gets_initial_cutoff",
    "unknown_user_returns_row_not_found",
)


def test_rust_auth_revocation_on_migrated_database(analytics_pg_url):
    environment = {
        **os.environ,
        "AUTH_STORE_TEST_DATABASE_URL": _libpq(os.environ["DATABASE_URL"]),
        "AUTH_STORE_TEST_OWNER_URL": _libpq(analytics_pg_url),
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "SQLX_OFFLINE": "true",
    }
    run = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--test", "auth_revocation_postgres",
            "--", "--include-ignored",
        ],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    for name in RUST_TESTS:
        assert f"test {name} ... ok" in run.stdout, run.stdout + run.stderr

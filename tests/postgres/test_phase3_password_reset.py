"""E1/E2 disposable database gate."""
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_e1_e2_postgres(analytics_pg_url):
    run = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
         "--features", "contract-tests", "--test", "auth_password_reset_postgres", "--", "--include-ignored"],
        cwd=ROOT,
        env={**os.environ, "AUTH_EMAIL_TEST_OWNER_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgresql://"), "AUTH_EMAIL_TEST_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgresql://"), "SQLX_OFFLINE": "true"},
        capture_output=True, text=True, timeout=1200,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert "test e2_consumes_and_revokes ... ok" in run.stdout
    assert "test e2_invited_account_retains_resolver_state ... ok" in run.stdout
    assert "test e1_write_attempt_restarts ... ok" in run.stdout
    assert "test e1_response_cleanup_and_delivery ... ok" in run.stdout

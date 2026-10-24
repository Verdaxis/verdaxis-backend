"""E1/E2 disposable database gate."""
import subprocess
from pathlib import Path

from tests.postgres.cargo_support import cargo_test_environment

ROOT = Path(__file__).resolve().parents[2]


def test_e1_e2_postgres(analytics_pg_url):
    run = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
         "--features", "contract-tests", "--test", "auth_password_reset_postgres", "--", "--include-ignored"],
        cwd=ROOT,
        env=cargo_test_environment(analytics_pg_url),
        capture_output=True, text=True, timeout=1200,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert "test e2_consumes_and_revokes ... ok" in run.stdout
    assert "test e2_invited_account_retains_resolver_state ... ok" in run.stdout
    assert "test e1_write_attempt_restarts ... ok" in run.stdout
    assert "test e1_response_cleanup_and_delivery ... ok" in run.stdout

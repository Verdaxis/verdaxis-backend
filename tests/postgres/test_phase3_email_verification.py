"""E3/E4 disposable database gate."""
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_e3_e4_postgres(analytics_pg_url):
    environment = {
        **os.environ,
        "AUTH_EMAIL_TEST_DATABASE_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgresql://"),
        "SQLX_OFFLINE": "true",
    }
    run = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
         "--features", "contract-tests", "--test", "auth_email_verification_postgres", "--", "--include-ignored"],
        cwd=ROOT, env=environment, capture_output=True, text=True, timeout=1200,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert "test e3_consumes_and_assigns_once ... ok" in run.stdout
    assert "test e4_response_and_delivery_boundary ... ok" in run.stdout

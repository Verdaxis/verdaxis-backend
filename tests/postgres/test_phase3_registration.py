"""E5/E6 disposable database gate."""
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_e5_e6_postgres(analytics_pg_url):
    environment = {
        **os.environ,
        "AUTH_EMAIL_TEST_OWNER_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgresql://"),
        "AUTH_EMAIL_TEST_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgresql://"),
        "SQLX_OFFLINE": "true",
    }
    run = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
         "--features", "contract-tests", "--test", "auth_registration_postgres", "--", "--include-ignored", "--test-threads=1"],
        cwd=ROOT, env=environment, capture_output=True, text=True, timeout=1200,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    for name in (
        "e5_response_and_delivery_boundary",
        "e6_pending_verification_and_single_consumption",
        "e5_e6_atomic_referral_registration",
        "e5_duplicate_registration_counts_referral_once",
        "e5_failed_delivery_preserves_response",
        "e6_concurrent_consumption_has_one_result",
        "e3_e6_pending_expiry_boundaries",
        "e5_e6_http_limits",
        "e5_e6_monitor_admission_controls_cleanup",
        "e5_e6_mail_logs_exclude_credentials",
        "e5_domain_monitor_cleanup",
        "e3_e6_reset_errors_exclude_credentials",
    ):
        assert f"test {name} ... ok" in run.stdout, run.stdout

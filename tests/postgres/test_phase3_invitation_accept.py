"""S6 invitation acceptance on the disposable migrated database.

Drives the Rust component against the real schema and the runtime role, then
runs the Python admin-invitation business rules that must still hold after the
cutover: acceptance is one-use, requires the terms and the password policy,
revokes every earlier session, advances referral attribution, and leaves the
Rust-owned reset table alone.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from tests.postgres.conftest import analytics_pg_url  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
CASES = (
    "invitation_accept_consumes_token_and_issues_one_family",
    "invitation_accept_revokes_all_user_families",
    "invitation_accept_requires_terms_and_strong_password",
    "invitation_accept_rejects_expired_or_reissued_token",
    "invitation_accept_rejects_ineligible_account_states",
    "invitation_accept_is_limited_and_origin_checked",
    "invitation_accept_does_not_touch_password_reset_tokens",
    "invitation_accept_concurrent_claims_yield_one_session",
)


def test_phase3_invitation_accept(analytics_pg_url):
    environment = {
        **os.environ,
        "AUTH_EMAIL_TEST_OWNER_URL": analytics_pg_url.replace(
            "postgresql+asyncpg://", "postgresql://"
        ),
        "AUTH_EMAIL_TEST_DATABASE_URL": os.environ["DATABASE_URL"].replace(
            "postgresql+asyncpg://", "postgresql://"
        ),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "RUST_TEST_PYTHON": sys.executable,
    }
    run = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests",
            "--test", "auth_invitation_accept_postgres",
            "--", "--include-ignored", "--test-threads=1",
        ],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert run.returncode == 0, run.stdout[-16000:] + run.stderr[-16000:]
    for name in CASES:
        assert f"test {name} ... ok" in run.stdout, run.stdout

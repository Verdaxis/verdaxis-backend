"""Operator preflight semantics that depend on PostgreSQL NULL behavior."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from uuid import uuid4

from app.models.user import User, UserRole, UserStatus


ROOT = Path(__file__).resolve().parents[2]


async def test_preflight_counts_approved_verified_user_without_organization_as_ineligible(
    pg_session,
    analytics_pg_url,
):
    _engine, session = pg_session
    session.add(
        User(
            id=uuid4(),
            email=f"no-org-{uuid4()}@example.test",
            password_hash="hash",
            role=UserRole.BUYER,
            status=UserStatus.APPROVED,
            email_verified=True,
            organization_id=None,
        )
    )
    await session.commit()

    completed = subprocess.run(
        [sys.executable, "-m", "scripts.security_preflight"],
        cwd=ROOT,
        env={**os.environ, "DATABASE_URL": analytics_pg_url},
        text=True,
        capture_output=True,
        check=False,
    )

    assert completed.returncode == 2, completed.stdout + completed.stderr
    assert "ineligible_users=1" in completed.stdout
    assert "enforcement_preflight=BLOCKED" in completed.stdout
    assert "kyc_enforcement=ADVISORY_HELD" in completed.stdout


async def test_preflight_does_not_block_an_unverified_pending_registration(
    pg_session,
    analytics_pg_url,
):
    _engine, session = pg_session
    session.add(
        User(
            id=uuid4(),
            email=f"pending-{uuid4()}@example.test",
            password_hash="hash",
            role=UserRole.BUYER,
            status=UserStatus.PENDING,
            email_verified=False,
            organization_id=None,
        )
    )
    await session.commit()

    completed = subprocess.run(
        [sys.executable, "-m", "scripts.security_preflight"],
        cwd=ROOT,
        env={**os.environ, "DATABASE_URL": analytics_pg_url},
        text=True,
        capture_output=True,
        check=False,
    )

    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert "ineligible_users=0" in completed.stdout
    assert "enforcement_preflight=READY" in completed.stdout

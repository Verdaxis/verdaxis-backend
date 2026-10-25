"""Phase 3 Task 14 (slimmed under method B): the session race suite wrapper.

Drives the Rust race cases on the disposable migrated database and adds the
one cross-writer cell that method B keeps: the Python auth-maintenance timer,
which still deletes expired refresh rows after the Phase 3 switch. Races whose
other writer was a Python login/refresh/logout/password route are dropped:
those routes stop serving in the one switch and never write these rows again.
"""
from __future__ import annotations

import asyncio
import os
import subprocess
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import hash_token_identifier
from app.services.auth_maintenance import run_auth_maintenance
from tests.postgres.conftest import analytics_pg_url  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]

# Every PostgreSQL race case in rs/read/tests/auth_races_postgres.rs. A
# skipped or missing case fails this suite.
RUST_RACE_CASES = (
    "r1_concurrent_refresh_same_token_yields_one_successor",
    "r2_refresh_replay_after_grace_revokes_family",
    "r3_refresh_vs_logout_revokes_successor",
    "r4_refresh_vs_password_change_never_issues_after_cutoff",
    "r6_login_vs_account_status_change_writes_no_session",
    "r7_device_session_limit_is_one_live_family",
    "r8_login_vs_login_different_devices_both_live",
    "r12_cutoff_downgrade_fails_fast",
)


def _libpq(url: str) -> str:
    return url.replace("postgresql+asyncpg://", "postgres://", 1)


def test_phase3_rust_session_races(analytics_pg_url):
    """Run the Rust single-stack race schedule on the migrated database."""
    environment = {
        **os.environ,
        "AUTH_EMAIL_TEST_OWNER_URL": _libpq(analytics_pg_url),
        "AUTH_EMAIL_TEST_DATABASE_URL": _libpq(os.environ["DATABASE_URL"]),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
    }
    run = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests",
            "--test", "auth_races_postgres",
            "--", "--include-ignored", "--test-threads=1",
        ],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert run.returncode == 0, run.stdout[-16000:] + run.stderr[-16000:]
    for name in RUST_RACE_CASES:
        assert f"test {name} ... ok" in run.stdout, run.stdout


@pytest.mark.asyncio
async def test_maintenance_deletion_races_a_live_rust_family(analytics_pg_url):
    """The Python maintenance timer may delete rows while Rust families live.

    The timer's selection (expired rows; revoked families past retention with
    no live successor) must never take a row a live Rust family still needs,
    even when the deletion runs between a Rust rotation and its observation.
    """
    engine = create_async_engine(os.environ["DATABASE_URL"])
    user = uuid4()
    live_family = uuid4()
    expired_family = uuid4()
    live_jti = "live-rust-" + uuid4().hex
    expired_jti = "expired-rust-" + uuid4().hex
    device = "D" + "d" * 42

    async with engine.begin() as connection:
        await connection.execute(
            text(
                "INSERT INTO users (id, email, password_hash, role, status, "
                "email_verified, must_change_password) "
                "VALUES (:id, :email, 'unused', 'BUYER', 'APPROVED', true, false)"
            ),
            {"id": user, "email": f"maintenance-race-{user}@example.com"},
        )
        await connection.execute(
            text(
                "INSERT INTO refresh_sessions "
                "(user_id, family_id, jti_hash, device_id_hash, revoked, expires_at, "
                "created_at) VALUES (:user, :live_family, :live_jti, :device, false, "
                "now() + interval '7 days', now())"
            ),
            {
                "user": user,
                "live_family": live_family,
                "live_jti": hash_token_identifier(live_jti),
                "device": hash_token_identifier(device),
            },
        )
        await connection.execute(
            text(
                "INSERT INTO refresh_sessions "
                "(user_id, family_id, jti_hash, device_id_hash, revoked, expires_at, "
                "created_at) VALUES (:user, :expired_family, :expired_jti, :device, "
                "false, now() - interval '1 second', now() - interval '1 minute')"
            ),
            {
                "user": user,
                "expired_family": expired_family,
                "expired_jti": hash_token_identifier(expired_jti),
                "device": hash_token_identifier(device),
            },
        )

    factory = async_sessionmaker(engine, class_=AsyncSession)
    # One maintenance pass concurrent with a rotation-shaped read: the pass
    # deletes only the expired row; the live family is untouched.
    async def rotate_shaped_read():
        async with factory() as session:
            rows = await session.execute(
                text(
                    "SELECT family_id, revoked FROM refresh_sessions "
                    "WHERE user_id=:id ORDER BY created_at"
                ),
                {"id": user},
            )
            return rows.all()

    async def maintenance_pass():
        async with factory() as session:
            return await run_auth_maintenance(session)

    read_result, counts = await asyncio.gather(rotate_shaped_read(), maintenance_pass())

    assert counts["refresh_sessions_deleted"] >= 1, counts
    families = {row[0] for row in read_result}
    assert live_family in families, "the live family was visible throughout"
    async with engine.connect() as connection:
        live = await connection.execute(
            text(
                "SELECT count(*) FROM refresh_sessions "
                "WHERE user_id=:id AND family_id=:family AND revoked=false"
            ),
            {"id": user, "family": live_family},
        )
        assert live.scalar_one() == 1, "maintenance took a live family row"
    async with engine.begin() as connection:
        await connection.execute(
            text("DELETE FROM refresh_sessions WHERE user_id=:id"), {"id": user}
        )
        await connection.execute(text("DELETE FROM users WHERE id=:id"), {"id": user})
    await engine.dispose()

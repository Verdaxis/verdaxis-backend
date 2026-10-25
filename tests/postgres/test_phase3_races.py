"""Single-stack races and the retained Python-maintenance writer seam."""
from __future__ import annotations

import asyncio
import os
import subprocess

import httpx
import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import async_sessionmaker

from app.services.auth_maintenance import run_auth_maintenance
from tests.postgres.test_phase3_cross_stack_auth import (
    ORIGIN,
    PASSWORD,
    ROOT,
    account,
    cargo,
    loopback_cookies,
)
from tests.postgres.test_phase3_cross_stack_auth import (
    rust_auth_server as rust_auth_server,
)

RUST_RACE_CASES = (
    "password_change_before_refresh_keeps_only_its_successor",
    "login_before_rejection_leaves_no_live_session",
    "invitation_accept_before_refresh_keeps_only_its_family",
    "password_reset_before_login_refuses_the_old_password",
    "r1_concurrent_refresh_same_token_yields_one_successor",
    "r2_refresh_replay_after_grace_revokes_family",
    "r3_refresh_vs_logout_revokes_successor",
    "r4_refresh_vs_password_change_never_issues_after_cutoff",
    "r5_refresh_vs_admin_revocation_never_issues_after_cutoff",
    "r6_login_vs_account_status_change_writes_no_session",
    "r7_device_session_limit_is_one_live_family",
    "r8_login_vs_login_different_devices_both_live",
    "r9_refresh_vs_invitation_accept_serializes",
    "r10_login_vs_password_reset_serializes",
    "r12_cutoff_downgrade_fails_fast",
)


def test_phase3_rust_session_races(analytics_pg_url):
    environment = {
        **os.environ,
        "AUTH_EMAIL_TEST_OWNER_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgres://", 1),
        "AUTH_EMAIL_TEST_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgres://", 1),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    }
    for _ in range(3):
        run = subprocess.run(
            cargo("test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
                  "--features", "contract-tests", "--test", "auth_races_postgres", "--test", "auth_coexistence_postgres",
                  "--", "--include-ignored", "--test-threads=1"),
            cwd=ROOT, env=environment, capture_output=True, text=True, timeout=1800,
        )
        assert run.returncode == 0, run.stdout[-16000:] + run.stderr[-16000:]
        for name in (*RUST_RACE_CASES, "python_and_rust_device_lock_contend_on_one_key"):
            assert f"test {name} ... ok" in run.stdout, run.stdout


async def wait_for_blocked(owner):
    async with asyncio.timeout(15):
        while True:
            async with owner.connect() as conn:
                if (await conn.execute(text("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE "
                                            "datname=current_database() AND cardinality(pg_blocking_pids(pid))>0)"))).scalar_one():
                    return
            await asyncio.sleep(0)


@pytest.mark.asyncio
@pytest.mark.parametrize("maintenance_first", [False, True])
async def test_maintenance_deletion_races_a_live_rust_family(
    analytics_pg_url, rust_auth_server, maintenance_first,
):
    async with account(analytics_pg_url) as (owner, runtime, user, _, email):
        async with httpx.AsyncClient(base_url=rust_auth_server, headers={"Origin": ORIGIN}, event_hooks={"response": [loopback_cookies]}) as rust:
            logged_in = await rust.post("/api/auth/login", data={"username": email, "password": PASSWORD})
            assert logged_in.status_code == 200
            # Numeric-loopback HTTP cannot return Secure cookies automatically.
            # Send the actual issued cookie values explicitly for this adapter.
            rust.headers["Cookie"] = "; ".join(f"{cookie.name}={cookie.value}" for cookie in rust.cookies.jar)
            # The maintenance writer has an expired row in the same family.
            # It must not remove the live predecessor or the new successor.
            async with owner.begin() as conn:
                await conn.execute(text("INSERT INTO refresh_sessions (id,user_id,family_id,jti_hash,device_id_hash,"
                                        "revoked,expires_at,created_at) SELECT gen_random_uuid(),user_id,family_id,"
                                        "md5(random()::text),device_id_hash,true,now()-interval '1 day',"
                                        "now()-interval '2 days' FROM refresh_sessions WHERE user_id=:id LIMIT 1"),
                                   {"id": user})

            async def maintenance():
                async with async_sessionmaker(runtime)() as session:
                    result = await run_auth_maintenance(session)
                    await session.commit()
                    return result

            async with owner.connect() as blocker:
                tx = await blocker.begin()
                try:
                    if maintenance_first:
                        await blocker.execute(text("SELECT id FROM refresh_sessions WHERE user_id=:id "
                                                   "AND expires_at<now() FOR UPDATE"), {"id": user})
                        pending = asyncio.create_task(maintenance())
                        await wait_for_blocked(owner)
                        rotated = await rust.post("/api/auth/refresh")
                        assert rotated.status_code == 200
                        await tx.commit()
                        counts = await pending
                    else:
                        await blocker.execute(text("SELECT id FROM users WHERE id=:id FOR UPDATE"), {"id": user})
                        pending = asyncio.create_task(rust.post("/api/auth/refresh"))
                        await wait_for_blocked(owner)
                        counts = await maintenance()
                        await tx.commit()
                        rotated = await pending
                        assert rotated.status_code == 200
                finally:
                    if tx.is_active:
                        await tx.rollback()
                    if "pending" in locals() and not pending.done():
                        await pending
            assert counts["refresh_sessions_deleted"] >= 1
            async with owner.connect() as conn:
                rows = (await conn.execute(text("SELECT revoked,replaced_by_jti_hash FROM refresh_sessions "
                                                "WHERE user_id=:id ORDER BY created_at"), {"id": user})).all()
            assert len(rows) == 2
            assert sum(not row.revoked for row in rows) == 1
            assert sum(row.replaced_by_jti_hash is not None for row in rows) == 1

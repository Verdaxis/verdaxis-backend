"""Serve the private market stream route on the role-separated disposable database.

The Rust cases connect as the application role for stream admission and replay,
and as the owner role only to write outbox rows and change account state. The
application URL is `DATABASE_URL`; a run whose URL logs in as any other role,
such as the migrator that owns the tables, is refused before the cases start.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

ROOT = Path(__file__).resolve().parents[2]
APP_ROLE = os.environ.get("RUNTIME_TEST_APP_ROLE", "verdaxis_app_test")

CASES = (
    "trades_get_alias_matches_canonical",
    "trades_database_admission_refusals",
)
MARKERS = (
    "header cursor replay is scoped to the organization",
    "live delivery follows replay without foreign events",
    "capacity is one in-band control; a dropped body frees its slot",
    "alias path, header cursor precedence and heartbeat",
    "query cursor replay",
    "account and organization refusals precede headers",
    "an unanswered account store is 503 before headers",
)


def _sqlx_url(url: str) -> str:
    return url.replace("postgresql+asyncpg://", "postgresql://", 1)


async def _require_app_role(url: str) -> None:
    """Refuse a URL that does not log in as the application role.

    The route must run with the runtime's authority: the configured application
    role, which neither owns the account table nor is a superuser.
    """
    engine = create_async_engine(url)
    try:
        async with engine.connect() as connection:
            row = (
                await connection.execute(
                    text(
                        "SELECT current_user::text AS role, r.rolsuper AS superuser, "
                        "(SELECT tableowner::text FROM pg_tables "
                        "WHERE schemaname = 'public' AND tablename = 'users') AS owner "
                        "FROM pg_roles r WHERE r.rolname = current_user"
                    )
                )
            ).one()
    finally:
        await engine.dispose()
    assert row.role == APP_ROLE, f"the stream route must connect as {APP_ROLE}, not {row.role}"
    assert row.role != row.owner, "the stream route must not connect as the table owner"
    assert not row.superuser, "the stream route must not connect as a superuser"


@pytest.mark.asyncio
async def test_the_role_check_refuses_the_table_owner(analytics_pg_url):
    # The suite's own URL is the migrator, which owns the tables.
    with pytest.raises(AssertionError, match="must connect as"):
        await _require_app_role(analytics_pg_url)


@pytest.mark.asyncio
async def test_private_stream_route_on_the_disposable_database(pg_session, analytics_pg_url):
    _, session = pg_session
    app_url = os.environ["DATABASE_URL"]
    await _require_app_role(app_url)
    organization, other, pending = uuid4(), uuid4(), uuid4()
    member, pending_member = uuid4(), uuid4()
    for organization_id, status in (
        (organization, "APPROVED"),
        (other, "APPROVED"),
        (pending, "PENDING"),
    ):
        await session.execute(
            text(
                "INSERT INTO organizations (id,name,type,verification_status) "
                "VALUES (:id,:name,'FUEL_BUYER',:status)"
            ),
            {"id": organization_id, "name": f"Stream fixture {organization_id}", "status": status},
        )
    for user_id, organization_id in ((member, organization), (pending_member, pending)):
        await session.execute(
            text(
                "INSERT INTO users (id,email,password_hash,role,status,organization_id,"
                "email_verified,must_change_password) VALUES "
                "(:id,:email,'unused','BUYER','APPROVED',:org,true,false)"
            ),
            {"id": user_id, "email": f"stream-{user_id}@example.test", "org": organization_id},
        )
    await session.commit()
    result = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests", "--test",
            "streams_http", "--", "--ignored", "--test-threads=1", "--nocapture",
        ],
        cwd=ROOT,
        env={
            **os.environ,
            "SQLX_OFFLINE": "true",
            "STREAM_HTTP_APP_URL": _sqlx_url(app_url),
            "STREAM_HTTP_OWNER_URL": _sqlx_url(analytics_pg_url),
            "STREAM_HTTP_USER": str(member),
            "STREAM_HTTP_ORG": str(organization),
            "STREAM_HTTP_OTHER_ORG": str(other),
            "STREAM_HTTP_PENDING_USER": str(pending_member),
            "STREAM_HTTP_PENDING_ORG": str(pending),
        },
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    output = result.stdout + result.stderr
    print(output[-8000:])
    assert result.returncode == 0, output
    assert f"test result: ok. {len(CASES)} passed; 0 failed; 0 ignored" in output, output
    # Test output interleaves with the result lines, so each case is named by
    # its start line and proved by its markers.
    for case in CASES:
        assert f"test {case} ..." in output, case
    for marker in MARKERS:
        assert f"stream http postgres: {marker}" in output, marker

"""Exercise Rust replay on the harness-owned database and app role."""
import os
import subprocess
from pathlib import Path
from uuid import UUID, uuid4

import pytest
import pytest_asyncio
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.retention_test_support import clear_retention

ROOT = Path(__file__).resolve().parents[2]


@pytest_asyncio.fixture(autouse=True)
async def isolated_replay_history(analytics_pg_url):
    # Other suites leave committed outbox rows on the shared disposable
    # database; the replay proof needs an empty retained history.
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.begin() as connection:
            await clear_retention(connection)
        yield
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_retained_replay_snapshots_and_live_admission(pg_session, analytics_pg_url):
    _, session = pg_session
    organization = UUID(int=7)
    user = uuid4()
    await session.execute(text(
        "INSERT INTO organizations (id,name,type,verification_status) VALUES "
        "(:id,'Replay fixture','FUEL_BUYER','APPROVED') ON CONFLICT (id) DO NOTHING"
    ), {"id": organization})
    await session.execute(text(
        "INSERT INTO users (id,email,password_hash,role,status,organization_id,"
        "email_verified,must_change_password) VALUES "
        "(:id,:email,'unused','BUYER','APPROVED',:org,true,false)"
    ), {"id": user, "email": f"replay-{user}@example.test", "org": organization})
    await session.commit()
    result = subprocess.run([
        "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p",
        "verdaxis-read", "--features", "contract-tests", "--test", "streams_replay",
        "--", "--ignored", "--exact", "postgres_replay_snapshots", "--nocapture",
    ], cwd=ROOT, env={
        **os.environ,
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "STREAM_REPLAY_APP_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgresql://", 1),
        "STREAM_REPLAY_OWNER_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgresql://", 1),
        "STREAM_REPLAY_USER": str(user),
    }, text=True, capture_output=True, timeout=1800, check=False)
    output = result.stdout + result.stderr
    assert result.returncode == 0, output
    assert "test result: ok. 1 passed; 0 failed; 0 ignored" in output, output
    for marker in (
        "601 authorized rows with foreign holes and released connections",
        "prune after captured page and before next snapshot",
        "empty retained table keeps floor and equal cursor",
        "current account cutoff terminates live delivery",
    ):
        assert marker in output, output

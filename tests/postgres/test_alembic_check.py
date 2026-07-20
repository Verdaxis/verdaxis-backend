"""Alembic autogenerate authority against clean PostgreSQL/PostGIS."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine


ROOT = Path(__file__).resolve().parents[2]


def _alembic_check(database_url: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "alembic", "check"],
        cwd=ROOT,
        env={**os.environ, "DATABASE_URL": database_url},
        text=True,
        capture_output=True,
        check=False,
    )


async def test_alembic_check_is_clean_with_postgis_extensions(analytics_pg_url):
    completed = _alembic_check(analytics_pg_url)
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert "No new upgrade operations detected" in completed.stdout


async def test_alembic_check_still_detects_real_public_application_drift(analytics_pg_url):
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.begin() as connection:
            await connection.execute(
                text("ALTER TABLE users ADD COLUMN alembic_public_drift_probe TEXT")
            )

        completed = _alembic_check(analytics_pg_url)
        output = completed.stdout + completed.stderr
        assert completed.returncode != 0, output
        assert "alembic_public_drift_probe" in output
    finally:
        async with engine.begin() as connection:
            await connection.execute(
                text("ALTER TABLE users DROP COLUMN IF EXISTS alembic_public_drift_probe")
            )
        await engine.dispose()

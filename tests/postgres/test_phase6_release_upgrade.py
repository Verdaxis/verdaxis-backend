"""Upgrade each stream checkpoint from the deployed cache-change revision."""
import asyncio

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.test_market_migrations import (  # noqa: F401
    _alembic,
    migration_database,
)


@pytest.mark.asyncio
async def test_stream_upgrade_from_cache_changes(migration_database):
    url, _ = migration_database
    for revision in ("prc_20261026_changes", "p3_20261020_auth_tokens", "sse_20261021_replay_floor"):
        result = await asyncio.to_thread(_alembic, url, "upgrade", revision)
        assert result.returncode == 0, result.stdout + result.stderr
        engine = create_async_engine(url)
        try:
            async with engine.connect() as connection:
                assert (await connection.execute(text("SELECT version_num FROM alembic_version"))).scalars().all() == [revision]
        finally:
            await engine.dispose()

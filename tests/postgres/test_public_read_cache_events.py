"""Transactional public cache metadata never grants runtime write authority."""
import pytest
from sqlalchemy import text


@pytest.mark.asyncio
async def test_committed_change_advances_generation_atomically(analytics_pg_url):
    from sqlalchemy.ext.asyncio import create_async_engine
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.connect() as connection:
            before = await connection.scalar(text("SELECT generation FROM public_read_cache_generations WHERE domain='trading'"))
            await connection.rollback()
            transaction = await connection.begin()
            await connection.execute(text("UPDATE products SET is_active=is_active"))
            during = await connection.scalar(text("SELECT generation FROM public_read_cache_generations WHERE domain='trading'"))
            assert during > before
            await transaction.rollback()
            assert await connection.scalar(text("SELECT generation FROM public_read_cache_generations WHERE domain='trading'")) == before
            await connection.rollback()
            async with connection.begin():
                await connection.execute(text("UPDATE products SET is_active=is_active"))
            assert await connection.scalar(text("SELECT generation FROM public_read_cache_generations WHERE domain='trading'")) > before
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_cache_trigger_functions_are_not_publicly_executable(analytics_pg_url):
    from sqlalchemy.ext.asyncio import create_async_engine
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.connect() as connection:
            assert await connection.scalar(text("SELECT count(*) FROM pg_proc WHERE proname='bump_public_read_cache_generation'")) == 1
            assert await connection.scalar(text("SELECT count(*) FROM pg_proc p, LATERAL aclexplode(p.proacl) a WHERE p.proname='bump_public_read_cache_generation' AND a.grantee=0 AND a.privilege_type='EXECUTE'")) == 0
    finally:
        await engine.dispose()


def test_rust_listener_uses_migrated_disposable_database(analytics_pg_url):
    import os
    import subprocess
    from pathlib import Path
    root = Path(__file__).resolve().parents[2]
    environment = {**os.environ, "PUBLIC_CACHE_TEST_DATABASE_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgres://"),
                   "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(), "SQLX_OFFLINE": "true"}
    subprocess.run(["cargo", "test", "--manifest-path", "rs/Cargo.toml", "-p", "verdaxis-read", "--test", "cache_events_postgres"],
                   cwd=root, env=environment, check=True, timeout=900)

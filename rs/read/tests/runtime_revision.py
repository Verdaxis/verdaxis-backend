"""Contract-only revision checks on an explicitly supplied disposable database."""
import asyncio
from pathlib import Path
import sys
from urllib.parse import urlsplit
import ipaddress
import os

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT))

# Check both targets before importing any database-bearing module.
for key in ("DATABASE_URL", "MIGRATOR_DATABASE_URL"):
    parsed = urlsplit(os.environ[key])
    assert ipaddress.ip_address(parsed.hostname).is_loopback
    assert parsed.path.endswith("_test") and not parsed.query
    os.environ[key] = parsed._replace(scheme="postgresql+asyncpg").geturl()

from app.config import settings
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine
from apply_migration_checkpoint import MigrationCheckpointError, verify_current_revision


async def main():
    engine = create_async_engine(settings.MIGRATOR_DATABASE_URL)
    # The disposable fixture owns this table. Restore its contents on exit.
    async with engine.connect() as connection:
        original = list((await connection.execute(text("SELECT version_num FROM alembic_version"))).scalars())
    try:
        for revisions in [[], ["kycg_20261014_staging", "unexpected"], ["kycg_20261014_staging"]]:
            async with engine.begin() as connection:
                await connection.execute(text("DELETE FROM alembic_version"))
                for revision in revisions:
                    await connection.execute(text("INSERT INTO alembic_version(version_num) VALUES (:revision)"), {"revision": revision})
            try:
                await verify_current_revision(settings, "kycg_20261014_staging")
            except MigrationCheckpointError:
                assert len(revisions) != 1
            else:
                assert revisions == ["kycg_20261014_staging"]
    finally:
        async with engine.begin() as connection:
            await connection.execute(text("DELETE FROM alembic_version"))
            for revision in original:
                await connection.execute(text("INSERT INTO alembic_version(version_num) VALUES (:revision)"), {"revision": revision})
        await engine.dispose()


asyncio.run(main())

"""Catalog and transaction checks for the read lookup index migration."""

from __future__ import annotations

import asyncio

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.test_market_migrations import (  # noqa: F401
    _alembic,
    _database_execute,
    migration_database,
)

PARENT = "p3_20261020_auth_tokens"
REVISION = "p5_20261020_read_indexes"
# pg_get_indexdef spells out uniqueness, access method, key order and any
# predicate, so one string pins the whole definition.
DEFINITIONS = {
    "ix_p5_producers_cod_name_id": (
        "CREATE INDEX ix_p5_producers_cod_name_id ON public.producer_projects "
        "USING btree (cod_year, name, id)"
    ),
    "ix_p5_producers_status_cod_id": (
        "CREATE INDEX ix_p5_producers_status_cod_id ON public.producer_projects "
        "USING btree (status, cod_year, id)"
    ),
    "ix_p5_vessels_org_id": (
        "CREATE INDEX ix_p5_vessels_org_id ON public.vessels "
        "USING btree (organization_id, id)"
    ),
}
INDEX_DEFINITIONS = (
    "SELECT indexname, indexdef FROM pg_indexes "
    "WHERE schemaname = 'public' AND indexname LIKE 'ix\\_p5\\_%'"
)
# Every public relation except indexes, with its grants.
RELATIONS = (
    "SELECT relname, relkind, relacl::text FROM pg_class "
    "WHERE relnamespace = 'public'::regnamespace AND relkind <> 'i' ORDER BY relname"
)
ROW_COUNTS = (
    "SELECT (SELECT count(*) FROM vessels), (SELECT count(*) FROM producer_projects)"
)


async def _indexes(database_url: str) -> dict[str, str]:
    result = await _database_execute(database_url, INDEX_DEFINITIONS)
    return dict(result.all())


async def _revision(database_url: str) -> str:
    result = await _database_execute(database_url, "SELECT version_num FROM alembic_version")
    return result.scalar_one()


async def _migrate(database_url: str, *arguments: str):
    return await asyncio.to_thread(_alembic, database_url, *arguments)


async def _populate(database_url: str) -> None:
    """Populated, nullable and tenant-scoped keys across the planned read caps."""
    await _database_execute(
        database_url,
        "INSERT INTO organizations (id, name, type) VALUES "
        "('00000000-0000-0000-0000-000000000501', 'Index fleet A', 'FUEL_BUYER'), "
        "('00000000-0000-0000-0000-000000000502', 'Index fleet B', 'FUEL_BUYER')",
    )
    await _database_execute(
        database_url,
        "INSERT INTO vessels (name, imo_number, organization_id) "
        "SELECT 'Index vessel ' || n, 'P5-INDEX-' || n, "
        "'00000000-0000-0000-0000-000000000501'::uuid "
        "FROM generate_series(1, 2001) AS n",
    )
    await _database_execute(
        database_url,
        "INSERT INTO vessels (name, imo_number, organization_id) VALUES "
        "('Other fleet vessel', 'P5-OTHER', '00000000-0000-0000-0000-000000000502'), "
        "('Unassigned vessel', 'P5-NULL', NULL)",
    )
    await _database_execute(
        database_url,
        "INSERT INTO producer_projects (name, fuel_type, country, cod_year, status) "
        "SELECT 'Index producer ' || n, 'METHANOL', 'NL', "
        "CASE WHEN n = 5001 THEN NULL ELSE 2027 END, "
        "CASE WHEN n % 2 = 0 THEN 'OPERATIONAL' ELSE 'ANNOUNCED' END "
        "FROM generate_series(1, 5001) AS n",
    )


async def _read_results(database_url: str) -> list[list]:
    """Compare results, not plans or timing, before and after the index build."""
    statements = (
        "SELECT id FROM vessels "
        "WHERE organization_id = '00000000-0000-0000-0000-000000000501' "
        "ORDER BY id LIMIT 2000",
        "SELECT id FROM vessels "
        "WHERE organization_id = '00000000-0000-0000-0000-000000000502' ORDER BY id",
        "SELECT id FROM vessels WHERE organization_id IS NULL ORDER BY id",
        "SELECT id FROM producer_projects ORDER BY cod_year, name, id LIMIT 5000",
        "SELECT id FROM producer_projects WHERE status = 'OPERATIONAL' "
        "AND cod_year >= 2027 ORDER BY cod_year, id",
        "SELECT id FROM producer_projects WHERE cod_year IS NULL ORDER BY name, id",
    )
    return [(await _database_execute(database_url, statement)).all() for statement in statements]


@pytest.mark.asyncio
async def test_phase5_index_catalog_definitions(analytics_pg_url):
    assert await _indexes(analytics_pg_url) == DEFINITIONS


@pytest.mark.asyncio
async def test_phase5_index_migration_refuses_behind_an_active_writer(migration_database):
    database_url, _database_name = migration_database
    upgraded = await _migrate(database_url, "upgrade", PARENT)
    assert upgraded.returncode == 0, upgraded.stderr
    await _populate(database_url)

    writer = create_async_engine(database_url)
    try:
        async with writer.connect() as connection:
            transaction = await connection.begin()
            # An open writer holds ROW EXCLUSIVE until it ends; the index build
            # must refuse at once rather than queue behind it.
            await connection.execute(text("LOCK TABLE producer_projects IN ROW EXCLUSIVE MODE"))
            refused = await _migrate(database_url, "upgrade", REVISION)
            await transaction.rollback()
    finally:
        await writer.dispose()

    assert refused.returncode != 0
    assert "could not obtain lock" in refused.stderr
    assert await _revision(database_url) == PARENT
    assert await _indexes(database_url) == {}


@pytest.mark.asyncio
async def test_phase5_index_migration_is_all_or_nothing(migration_database):
    database_url, _database_name = migration_database
    upgraded = await _migrate(database_url, "upgrade", PARENT)
    assert upgraded.returncode == 0, upgraded.stderr
    await _populate(database_url)
    before = (await _database_execute(database_url, RELATIONS)).all()
    rows = (await _database_execute(database_url, ROW_COUNTS)).one()
    results = await _read_results(database_url)
    assert rows == (2003, 5001)
    assert [len(result) for result in results] == [2000, 1, 1, 5000, 2500, 1]

    # A relation that takes the last index's name makes that build fail after
    # the earlier indexes were built in the same transaction.
    await _database_execute(database_url, "CREATE TABLE ix_p5_producers_status_cod_id (id integer)")
    failed = await _migrate(database_url, "upgrade", REVISION)
    assert failed.returncode != 0
    assert "already exists" in failed.stderr
    assert await _revision(database_url) == PARENT
    assert await _indexes(database_url) == {}
    await _database_execute(database_url, "DROP TABLE ix_p5_producers_status_cod_id")

    upgraded = await _migrate(database_url, "upgrade", REVISION)
    assert upgraded.returncode == 0, upgraded.stderr
    assert await _revision(database_url) == REVISION
    assert await _indexes(database_url) == DEFINITIONS
    # Indexes add no table, change no grant and keep every row.
    assert (await _database_execute(database_url, RELATIONS)).all() == before
    assert (await _database_execute(database_url, ROW_COUNTS)).one() == rows
    assert await _read_results(database_url) == results

    downgraded = await _migrate(database_url, "downgrade", PARENT)
    assert downgraded.returncode == 0, downgraded.stderr
    assert await _indexes(database_url) == {}
    assert (await _database_execute(database_url, ROW_COUNTS)).one() == rows
    assert await _read_results(database_url) == results

"""Cache authority across two disposable database incarnations.

Nothing here promotes or restores a live database. The analytics and market
databases are two migrated incarnations on one disposable server: the Rust tests
move the externally active authority between them, and the owner script is
proved against the real incarnation row.
"""
import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

ROOT = Path(__file__).resolve().parents[2]


def _owner():
    spec = importlib.util.spec_from_file_location(
        "rust_cache_authority", ROOT / "scripts/rust_cache_authority.py"
    )
    module = importlib.util.module_from_spec(spec)
    # dataclasses resolve string annotations through the module registry.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


owner = _owner()


async def _no_sleep(_seconds):
    return None


def _libpq(url: str) -> str:
    return url.replace("postgresql+asyncpg://", "postgres://")


@pytest.fixture
async def connection(analytics_pg_url):
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.connect() as connection:
            yield connection
            await connection.rollback()
            async with connection.begin():
                await connection.execute(
                    text(
                        "UPDATE public.public_read_cache_incarnation "
                        "SET active = false, authority_revision = 0"
                    )
                )
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_owner_activation_binds_the_real_incarnation_row(connection, tmp_path):
    store = owner.SqlIncarnationStore(connection)
    before = await store.read()
    assert before.in_recovery is False
    first = await owner.activate(tmp_path, store, sleep=_no_sleep)
    row = await store.read()
    assert (row.database_epoch, row.primary_identity) == (
        first.database_epoch,
        first.primary_identity,
    )
    assert row.authority_revision == first.revision
    assert row.active is True
    record = owner.read_record(tmp_path)
    assert record == owner.Record(first.revision, first)
    # A second activation, as after another restore, rotates everything and only
    # ever moves the revision up.
    second = await owner.activate(tmp_path, store, sleep=_no_sleep)
    assert second.revision > first.revision
    assert second.database_epoch != first.database_epoch
    assert second.primary_identity != first.primary_identity
    assert owner.read_record(tmp_path) == owner.Record(second.revision, second)


@pytest.mark.asyncio
async def test_binding_refuses_a_copied_identity_or_a_lower_revision(connection):
    store = owner.SqlIncarnationStore(connection)
    bound = owner.Authority(owner.new_identity(), owner.new_identity(), 5)
    await store.bind(bound)
    for refused in (
        owner.Authority(owner.new_identity(), owner.new_identity(), 5),
        owner.Authority(owner.new_identity(), owner.new_identity(), 4),
        owner.Authority(bound.database_epoch, owner.new_identity(), 6),
        owner.Authority(owner.new_identity(), bound.primary_identity, 6),
    ):
        with pytest.raises(owner.AuthorityError):
            await store.bind(refused)
    row = await store.read()
    assert (row.database_epoch, row.primary_identity, row.authority_revision) == (
        bound.database_epoch,
        bound.primary_identity,
        5,
    )


@pytest.mark.asyncio
async def test_a_failed_activation_leaves_the_record_revoked(connection, tmp_path, monkeypatch):
    store = owner.SqlIncarnationStore(connection)
    original = store.bind

    async def failing_bind(authority):
        await original(authority)
        raise owner.AuthorityError("simulated loss after commit")

    monkeypatch.setattr(store, "bind", failing_bind)
    with pytest.raises(owner.AuthorityError):
        await owner.activate(tmp_path, store, sleep=_no_sleep)
    # The database may be bound, but the record never named it: caching stays off.
    assert owner.read_record(tmp_path).authority is None


def test_rust_failover_scenarios_hold_on_two_database_incarnations(
    analytics_pg_url, market_pg_url
):
    environment = {
        **os.environ,
        "PUBLIC_CACHE_TEST_DATABASE_URL": _libpq(analytics_pg_url),
        "PUBLIC_CACHE_TEST_SECOND_DATABASE_URL": _libpq(market_pg_url),
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "RUST_TEST_PYTHON": sys.executable,
        "SQLX_OFFLINE": "true",
    }
    subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "-p", "verdaxis-read",
            "--test", "cache_failover_postgres", "--", "--test-threads=1",
        ],
        cwd=ROOT, env=environment, check=True, timeout=1200,
    )

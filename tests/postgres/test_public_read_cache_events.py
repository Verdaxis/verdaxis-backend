"""Transactional public cache metadata never grants runtime write authority."""
import asyncio
import importlib.util
import json
import os
import re
import secrets
import subprocess
import time
from pathlib import Path

import pytest
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.test_market_routes import _headers, route_market  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
GENERATION = "SELECT generation FROM public_read_cache_generations WHERE domain='trading'"
CACHE_TABLES = (
    "public_read_cache_generations",
    "public_read_cache_events",
    "public_read_cache_incarnation",
)

# Tables that Rust source names but that no trading cache entry depends on. Each
# needs a reason; an unreviewed table named by a Rust query fails the ledger test.
REVIEWED_EXCLUSIONS = {
    "news_items": "reference domain: the separate 60-second reference cache",
    "ports": "reference domain: the separate 60-second reference cache",
    "port_intelligence": "reference domain: the separate 60-second reference cache",
    "notifications": "private authenticated path; never cached",
    "referrals": "private authenticated path; never cached",
    "watchlists": "private authenticated path; never cached",
    "alembic_version": "revision attestation at startup, not a response dependency",
    **{table: "cache infrastructure itself" for table in CACHE_TABLES},
}


def _migration_dependencies():
    path = ROOT / "alembic/versions/prc_20261018_cache.py"
    spec = importlib.util.spec_from_file_location("prc_20261018_cache", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.DEPENDENCIES


DEPENDENCIES = _migration_dependencies()


# A table reference in embedded SQL follows a clause keyword.
_TABLE_CLAUSE = r"(?:from|join|update|into|table|lateral)\s+(?:only\s+)?(?:public\.)?\"?"


def _table_is_referenced(table: str) -> bool:
    """Whether a Rust SQL file or an embedded Rust query names this table.

    SQL files are scanned for the bare name (comments removed). Rust source only
    counts the name after a clause keyword, so a variable called `orders` or the
    word "contracts" in a comment is not mistaken for a table.
    """
    word = rf"(?<![A-Za-z0-9_]){re.escape(table)}(?![A-Za-z0-9_])"
    sql = "\n".join(
        re.sub(r"--[^\n]*", "", path.read_text())
        for path in sorted((ROOT / "rs/read/sql").rglob("*.sql"))
    )
    rust = "\n".join(
        re.sub(r"//[^\n]*", "", path.read_text())
        for path in sorted((ROOT / "rs/read/src").rglob("*.rs"))
    )
    return bool(
        re.search(word, sql)
        or re.search(rf"(?i){_TABLE_CLAUSE}{word}", rust)
        or re.search(rf"(?i){_TABLE_CLAUSE}{word}", sql)
    )


async def _generation(connection) -> int:
    return await connection.scalar(text(GENERATION))


@pytest.fixture
async def engine(analytics_pg_url):
    engine = create_async_engine(analytics_pg_url)
    try:
        yield engine
    finally:
        await engine.dispose()


@pytest.fixture
async def runtime_engine(analytics_pg_url):
    """The deployed application role: SELECT-only on cache metadata."""
    url = os.environ.get("DATABASE_URL", "")
    expected = os.environ.get("RUNTIME_TEST_APP_ROLE", "")
    if not url.startswith("postgresql") or not expected:
        pytest.skip("the application-role DATABASE_URL is not configured")
    engine = create_async_engine(url)
    try:
        async with engine.connect() as connection:
            assert await connection.scalar(text("SELECT current_user")) == expected
        yield engine
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_committed_change_advances_generation_atomically(engine):
    async with engine.connect() as connection:
        before = await _generation(connection)
        await connection.rollback()
        transaction = await connection.begin()
        await connection.execute(text("UPDATE products SET is_active=is_active"))
        during = await _generation(connection)
        assert during > before
        await transaction.rollback()
        assert await _generation(connection) == before
        await connection.rollback()
        async with connection.begin():
            await connection.execute(text("UPDATE products SET is_active=is_active"))
        assert await _generation(connection) > before


@pytest.mark.asyncio
async def test_cache_trigger_functions_are_not_publicly_executable(engine):
    async with engine.connect() as connection:
        assert await connection.scalar(text("SELECT count(*) FROM pg_proc WHERE proname='bump_public_read_cache_generation'")) == 1
        assert await connection.scalar(text("SELECT count(*) FROM pg_proc p, LATERAL aclexplode(p.proacl) a WHERE p.proname='bump_public_read_cache_generation' AND a.grantee=0 AND a.privilege_type='EXECUTE'")) == 0


@pytest.mark.asyncio
async def test_every_dependency_carries_the_always_enabled_statement_trigger(engine):
    async with engine.connect() as connection:
        rows = (
            await connection.execute(
                text(
                    "SELECT c.relname, tg.tgenabled::text AS enabled, tg.tgtype & 127 AS shape, "
                    "tg.tgfoid::regproc::text AS function_name "
                    "FROM pg_trigger tg JOIN pg_class c ON c.oid = tg.tgrelid "
                    "WHERE tg.tgname = 'public_read_cache_changed' AND NOT tg.tgisinternal"
                )
            )
        ).all()
    assert {row.relname for row in rows} == set(DEPENDENCIES)
    for row in rows:
        # After, per statement, INSERT|DELETE|UPDATE|TRUNCATE, enabled for every
        # session role, and the one reviewed trigger function.
        assert (row.enabled, row.shape) == ("A", 60), row.relname
        assert row.function_name.endswith("bump_public_read_cache_generation"), row.relname


@pytest.mark.asyncio
@pytest.mark.parametrize("table", DEPENDENCIES)
async def test_each_statement_kind_on_each_dependency_advances_the_generation(engine, table):
    async with engine.connect() as connection:
        column = await connection.scalar(
            text(
                "SELECT column_name FROM information_schema.columns "
                "WHERE table_schema = 'public' AND table_name = :table "
                "ORDER BY ordinal_position LIMIT 1"
            ),
            {"table": table},
        )
        await connection.rollback()
        for statement in (
            f'INSERT INTO public."{table}" ("{column}") SELECT "{column}" FROM public."{table}" WHERE false',
            f'UPDATE public."{table}" SET "{column}" = "{column}" WHERE false',
            f'DELETE FROM public."{table}" WHERE false',
        ):
            transaction = await connection.begin()
            before = await _generation(connection)
            await connection.execute(text(statement))
            # A statement trigger fires even when no row matches, once.
            assert await _generation(connection) == before + 1, statement
            await transaction.rollback()
        transaction = await connection.begin()
        before = await _generation(connection)
        await connection.execute(text(f'TRUNCATE public."{table}" CASCADE'))
        assert await _generation(connection) > before, table
        await transaction.rollback()


@pytest.mark.asyncio
async def test_concurrent_writers_get_distinct_contiguous_generations(engine):
    writers = 12
    async with engine.connect() as probe:
        before = await _generation(probe)
        await probe.rollback()

        async def write():
            async with engine.connect() as connection:
                async with connection.begin():
                    await connection.execute(text("DELETE FROM public.benchmarks WHERE false"))
                    # Hold the generation row so the writers genuinely overlap.
                    await connection.execute(text("SELECT pg_sleep(0.05)"))

        await asyncio.gather(*(write() for _ in range(writers)))
        after = await _generation(probe)
        events = (
            await probe.execute(
                text("SELECT generation FROM public_read_cache_events WHERE domain='trading' AND generation > :before ORDER BY generation"),
                {"before": before},
            )
        ).scalars().all()
    assert after == before + writers
    assert events == list(range(before + 1, after + 1))


@pytest.mark.asyncio
async def test_every_database_table_read_by_rust_is_a_dependency_or_reviewed(engine):
    async with engine.connect() as connection:
        tables = set(
            (
                await connection.execute(
                    text(
                        "SELECT table_name FROM information_schema.tables "
                        "WHERE table_schema = 'public' AND table_type = 'BASE TABLE'"
                    )
                )
            ).scalars()
        )
    assert set(DEPENDENCIES) <= tables
    mentioned = {table for table in tables if _table_is_referenced(table)}
    unreviewed = mentioned - set(DEPENDENCIES) - set(REVIEWED_EXCLUSIONS)
    assert not unreviewed, f"a Rust query names tables outside the cache ledger: {sorted(unreviewed)}"
    assert not set(DEPENDENCIES) & set(REVIEWED_EXCLUSIONS)


def test_rust_dependency_list_is_the_migration_list():
    source = (ROOT / "rs/read/src/runtime/cache_events.rs").read_text()
    block = re.search(r"TRADING_DEPENDENCY_TABLES: \[&str; (\d+)\] = \[(.*?)\];", source, re.DOTALL)
    assert block is not None
    names = re.findall(r'"([a-z_]+)"', block.group(2))
    assert int(block.group(1)) == len(names)
    assert tuple(names) == tuple(DEPENDENCIES)


def test_dependency_documentation_names_every_dependency():
    document = (ROOT / "docs/rust-phase1/cache-dependencies.md").read_text()
    for table in DEPENDENCIES:
        assert table in document, table


@pytest.mark.asyncio
async def test_runtime_role_reads_but_never_writes_cache_metadata(runtime_engine):
    statements = {
        "public_read_cache_generations": {
            "INSERT": "INSERT INTO public.public_read_cache_generations(domain, generation) VALUES ('x', 1)",
            "UPDATE": "UPDATE public.public_read_cache_generations SET generation = generation WHERE false",
            "DELETE": "DELETE FROM public.public_read_cache_generations WHERE false",
            "TRUNCATE": "TRUNCATE public.public_read_cache_generations",
        },
        "public_read_cache_events": {
            "INSERT": "INSERT INTO public.public_read_cache_events(domain, generation) VALUES ('x', 1)",
            "UPDATE": "UPDATE public.public_read_cache_events SET generation = generation WHERE false",
            "DELETE": "DELETE FROM public.public_read_cache_events WHERE false",
            "TRUNCATE": "TRUNCATE public.public_read_cache_events",
        },
        "public_read_cache_incarnation": {
            "INSERT": "INSERT INTO public.public_read_cache_incarnation(singleton, database_epoch, primary_identity) VALUES (false, 'a', 'b')",
            "UPDATE": "UPDATE public.public_read_cache_incarnation SET active = true WHERE false",
            "DELETE": "DELETE FROM public.public_read_cache_incarnation WHERE false",
            "TRUNCATE": "TRUNCATE public.public_read_cache_incarnation",
        },
    }
    for table, denied in statements.items():
        async with runtime_engine.connect() as connection:
            await connection.execute(text(f"SELECT * FROM public.{table}"))
        for statement in denied.values():
            async with runtime_engine.connect() as connection:
                with pytest.raises(DBAPIError, match="permission denied"):
                    await connection.execute(text(statement))


@pytest.mark.asyncio
async def test_runtime_role_cannot_bypass_or_remove_the_triggers(runtime_engine):
    for statement in (
        "ALTER TABLE public.products DISABLE TRIGGER public_read_cache_changed",
        "DROP TRIGGER public_read_cache_changed ON public.products",
        "SET session_replication_role = replica",
    ):
        async with runtime_engine.connect() as connection:
            with pytest.raises(DBAPIError):
                await connection.execute(text(statement))


@pytest.mark.asyncio
async def test_runtime_business_write_and_generation_append_share_one_transaction(
    runtime_engine, engine
):
    async with runtime_engine.connect() as connection:
        before = await _generation(connection)
        await connection.rollback()
        transaction = await connection.begin()
        # The app role has no authority on cache metadata; the trigger function
        # owns the write, inside the business statement's own transaction.
        await connection.execute(text("UPDATE public.orderbook_orders SET status = status WHERE false"))
        during = await _generation(connection)
        appended = await connection.scalar(
            text("SELECT count(*) FROM public_read_cache_events WHERE domain='trading' AND generation = :g"),
            {"g": during},
        )
        assert (during, appended) == (before + 1, 1)
        await transaction.rollback()
        assert await _generation(connection) == before
        assert await connection.scalar(
            text("SELECT count(*) FROM public_read_cache_events WHERE domain='trading' AND generation = :g"),
            {"g": during},
        ) == 0
        await connection.rollback()
        async with connection.begin():
            await connection.execute(text("UPDATE public.orderbook_orders SET status = status WHERE false"))
        assert await _generation(connection) == before + 1
    async with engine.connect() as connection:
        assert await _generation(connection) == before + 1


@pytest.mark.asyncio
async def test_normal_python_economic_writers_each_advance_the_generation(route_market):
    client, seeded = route_market
    engine = seeded["engine"]

    async def generation() -> int:
        async with engine.connect() as connection:
            return await _generation(connection)

    first = await generation()

    async def advanced(action, label):
        before = await generation()
        response = await action()
        after = await generation()
        assert after > before, label
        return response

    created = await advanced(
        lambda: client.post(
            "/api/orderbook",
            json={
                "side": "BID",
                "product_id": str(seeded["product_id"]),
                "delivery_point_id": str(seeded["point_id"]),
                "quantity_mt": "200.00",
                "price_per_mt_usd": "600.00",
                "availability_window": "SPOT",
            },
            headers=_headers(seeded["buyer_id"], "cache-writer-create"),
        ),
        "place order",
    )
    assert created.status_code == 201, created.text
    order_id = created.json()["id"]
    amended = await advanced(
        lambda: client.put(
            f"/api/orderbook/{order_id}",
            json={"price_per_mt_usd": "610.00"},
            headers=_headers(seeded["buyer_id"], "cache-writer-amend"),
        ),
        "amend order",
    )
    assert amended.status_code == 200, amended.text
    cancelled = await advanced(
        lambda: client.delete(
            f"/api/orderbook/{order_id}",
            headers=_headers(seeded["buyer_id"], "cache-writer-cancel"),
        ),
        "cancel order",
    )
    assert cancelled.status_code == 204, cancelled.text
    published = await advanced(
        lambda: client.post(
            f"/api/inventory/{seeded['inventory_id']}/publish",
            headers=_headers(seeded["seller_id"], "cache-writer-publish"),
        ),
        "publish ask",
    )
    assert published.status_code == 200, published.text
    crossed = await advanced(
        lambda: client.post(
            "/api/orderbook",
            json={
                "side": "BID",
                "product_id": str(seeded["product_id"]),
                "delivery_point_id": str(seeded["point_id"]),
                "quantity_mt": "300.00",
                "price_per_mt_usd": "700.00",
                "availability_window": "SPOT",
            },
            headers=_headers(seeded["buyer_id"], "cache-writer-cross"),
        ),
        "cross into a trade",
    )
    assert crossed.status_code == 201, crossed.text
    async with engine.connect() as connection:
        assert await connection.scalar(text("SELECT count(*) FROM trades")) == 1
        events = (
            await connection.execute(
                text("SELECT generation FROM public_read_cache_events WHERE domain='trading' AND generation > :first ORDER BY generation"),
                {"first": first},
            )
        ).scalars().all()
        assert events == list(range(first + 1, await _generation(connection) + 1))


def test_rust_listener_uses_migrated_disposable_database(analytics_pg_url):
    root = ROOT
    environment = {**os.environ, "PUBLIC_CACHE_TEST_DATABASE_URL": analytics_pg_url.replace("postgresql+asyncpg://", "postgres://"),
                   "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(), "SQLX_OFFLINE": "true"}
    subprocess.run(["cargo", "test", "--manifest-path", "rs/Cargo.toml", "-p", "verdaxis-read", "--test", "cache_events_postgres"],
                   cwd=root, env=environment, check=True, timeout=900)


def test_rust_trading_cache_needs_every_authority_and_generation(
    analytics_pg_url, market_pg_url
):
    """Run the Rust trading-cache target against two disposable incarnations.

    The two databases are separate migrated schemas on one disposable server,
    not live hosts. `an_entry_from_the_prior_incarnation_is_never_a_hit` needs
    both URLs, so this driver is what actually executes it in the suite.
    """
    environment = {
        **os.environ,
        "PUBLIC_CACHE_TEST_DATABASE_URL": analytics_pg_url.replace(
            "postgresql+asyncpg://", "postgres://", 1
        ),
        "PUBLIC_CACHE_TEST_SECOND_DATABASE_URL": market_pg_url.replace(
            "postgresql+asyncpg://", "postgres://", 1
        ),
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "SQLX_OFFLINE": "true",
    }
    subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            "rs/Cargo.toml",
            "-p",
            "verdaxis-read",
            "--test",
            "trading_cache_postgres",
            "--",
            "--test-threads=1",
        ],
        cwd=ROOT,
        env=environment,
        check=True,
        timeout=1200,
    )


async def _wait_for_file(path: Path, timeout: float = 180.0) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists():
        if time.monotonic() > deadline:
            raise AssertionError(f"timed out waiting for {path.name}")
        await asyncio.sleep(0.05)


@pytest.mark.asyncio
@pytest.mark.parametrize("incarnation", ["market", "analytics"])
async def test_two_rust_readers_never_serve_the_prior_generation(
    route_market, market_pg_url, analytics_pg_url, tmp_path, incarnation
):
    """Mixed Python-writer/Rust-reader schedule on two database incarnations.

    Two independent Rust processes on the same disposable incarnation warm a
    trading entry at generation G. Python then commits one real dependency write.
    Repeat the schedule on the other incarnation. Neither Rust process
    consumes the invalidation wake before its next read, so each must derive
    G+1 from the generation check in its own snapshot. Only then is the wake
    delivered and the entry cleared. This is a correctness schedule; it
    measures nothing.
    """
    client, seeded = route_market
    analytics = analytics_pg_url.replace("postgresql+asyncpg://", "postgres://", 1)
    engine = seeded["engine"]
    analytics_engine = create_async_engine(analytics_pg_url)
    epoch = secrets.token_hex(32)
    primary = secrets.token_hex(32)
    target = analytics if incarnation == "analytics" else market_pg_url.replace(
        "postgresql+asyncpg://", "postgres://", 1
    )
    readers = {"a": target, "b": target}
    # Each reader asserts its own database name, so the two incarnations are
    # proved rather than assumed.
    names = {reader: url.rsplit("/", 1)[-1].split("?", 1)[0] for reader, url in readers.items()}

    for database in (engine, analytics_engine):
        async with database.begin() as connection:
            await connection.execute(
                text(
                    "UPDATE public_read_cache_incarnation SET database_epoch = :epoch, "
                    "primary_identity = :primary, authority_revision = 1, active = true "
                    "WHERE singleton"
                ),
                {"epoch": epoch, "primary": primary},
            )
    record = tmp_path / "cache-authority.json"
    record.write_text(
        json.dumps(
            {
                "version": 1,
                "state": "active",
                "database_epoch": epoch,
                "primary_identity": primary,
                "authority_revision": 1,
            }
        )
    )

    root = ROOT
    release = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    environment = {
        **os.environ,
        "PUBLIC_CACHE_TEST_DATABASE_URL": readers["b"],
        "PUBLIC_CACHE_TEST_RECORD": str(record),
        "PUBLIC_CACHE_TEST_SCHEDULE": "mixed",
        "PUBLIC_CACHE_TEST_BARRIER_DIR": str(tmp_path),
        "RELEASE_SHA": release,
        "SQLX_OFFLINE": "true",
    }
    # Build once so the two readers do not contend for the cargo lock.
    subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            "rs/Cargo.toml",
            "-p",
            "verdaxis-read",
            "--test",
            "trading_cache_postgres",
            "--no-run",
        ],
        cwd=root,
        env=environment,
        check=True,
        timeout=900,
    )

    processes = {}
    logs = []
    try:
        for reader in ("a", "b"):
            log = (tmp_path / f"reader_{reader}.log").open("wb")
            logs.append(log)
            processes[reader] = await asyncio.create_subprocess_exec(
                "cargo",
                "test",
                "--manifest-path",
                "rs/Cargo.toml",
                "-p",
                "verdaxis-read",
                "--test",
                "trading_cache_postgres",
                "--",
                "--exact",
                "--nocapture",
                "mixed_python_writer_schedule",
                cwd=root,
                env={
                    **environment,
                    "PUBLIC_CACHE_TEST_READER": reader,
                    "PUBLIC_CACHE_TEST_READER_DATABASE_URL": readers[reader],
                    "PUBLIC_CACHE_TEST_EXPECT_DATABASE": names[reader],
                },
                stdout=log,
                stderr=asyncio.subprocess.STDOUT,
            )
        for reader in processes:
            await _wait_for_file(tmp_path / f"{reader}.warm.rust")

        created = await client.post(
            "/api/orderbook",
            json={
                "side": "BID",
                "product_id": str(seeded["product_id"]),
                "delivery_point_id": str(seeded["point_id"]),
                "quantity_mt": "200.00",
                "price_per_mt_usd": "600.00",
                "availability_window": "SPOT",
            },
            headers=_headers(seeded["buyer_id"], "cache-race-writer"),
        )
        assert created.status_code == 201, created.text
        if incarnation == "analytics":
            async with analytics_engine.begin() as connection:
                await connection.execute(text("UPDATE products SET is_active = is_active"))
        for reader in processes:
            (tmp_path / f"{reader}.warm.python").write_text("1")

        for reader in processes:
            await _wait_for_file(tmp_path / f"{reader}.read.rust")
            result = json.loads((tmp_path / f"result_{reader}.json").read_text())
            assert result["after"] > result["before"], (reader, result)
            await _wait_for_file(tmp_path / f"{reader}.cleared.rust")
            code = await asyncio.wait_for(processes[reader].wait(), 120)
            assert code == 0, (
                (tmp_path / f"reader_{reader}.log").read_text()
                if (tmp_path / f"reader_{reader}.log").exists()
                else reader
            )
    finally:
        for process in processes.values():
            if process.returncode is None:
                process.kill()
                await process.wait()
        for log in logs:
            log.close()
        for database in (engine, analytics_engine):
            async with database.begin() as connection:
                await connection.execute(
                    text(
                        "UPDATE public_read_cache_incarnation SET active = false, "
                        "authority_revision = 0"
                    )
                )
        await analytics_engine.dispose()

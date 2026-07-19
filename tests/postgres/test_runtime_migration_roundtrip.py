"""Real PG17 coverage for the runtime metadata downgrade policy."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest
from sqlalchemy import inspect, text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import create_async_engine


_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_PREVIOUS_REVISION = "pa_20260715_analytics_facts"
_TOUCHED_TABLES = (
    "audit_logs",
    "commissions",
    "inventory_items",
    "match_suggestions",
    "negotiations",
    "orderbook_orders",
    "producer_projects",
    "referrals",
    "rfq_quotes",
    "rfqs",
    "trades",
    "users",
)


def _alembic(*arguments: str, url: str | None = None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    if url is not None:
        env["DATABASE_URL"] = url
        env["MIGRATOR_DATABASE_URL"] = url
    return subprocess.run(
        [sys.executable, "-m", "alembic", *arguments],
        cwd=_BACKEND_ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def _admin_sql(statement: str, *, database: str = "postgres") -> None:
    url = make_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
    result = subprocess.run(
        [
            "psql",
            "-X",
            "-h",
            url.host or "",
            "-p",
            str(url.port or 5432),
            "-U",
            url.username or "",
            "-d",
            database,
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            statement,
        ],
        env={**os.environ, "PGPASSWORD": url.password or ""},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.asyncio
async def test_downgrade_restores_exact_previous_schema_and_match_id_not_null(
    analytics_pg_url,
):
    baseline_database = "verdaxis_schema_baseline_analytics_test"
    migrator_role = os.environ["RUNTIME_TEST_MIGRATOR_ROLE"]
    baseline_url = make_url(analytics_pg_url).set(database=baseline_database).render_as_string(
        hide_password=False
    )
    _admin_sql(f"DROP DATABASE IF EXISTS {baseline_database} WITH (FORCE)")
    _admin_sql(f"CREATE DATABASE {baseline_database} OWNER {migrator_role}")
    _admin_sql("CREATE EXTENSION postgis", database=baseline_database)
    previous = _alembic("upgrade", _PREVIOUS_REVISION, url=baseline_url)
    assert previous.returncode == 0, previous.stderr

    engine = create_async_engine(analytics_pg_url)
    try:
        async def schema_snapshot(url: str):
            snapshot_engine = create_async_engine(url)
            try:
                async with snapshot_engine.connect() as connection:
                    def inspect_schema(sync_connection):
                        inspector = inspect(sync_connection)
                        return {
                            table: {
                                "columns": tuple(
                                    (
                                        column["name"],
                                        str(column["type"]),
                                        column["nullable"],
                                        str(column["default"]),
                                    )
                                    for column in inspector.get_columns(
                                        table, schema="public"
                                    )
                                ),
                                "indexes": tuple(
                                    sorted(
                                        (
                                            index["name"],
                                            tuple(index["column_names"]),
                                            index["unique"],
                                        )
                                        for index in inspector.get_indexes(
                                            table, schema="public"
                                        )
                                    )
                                ),
                            }
                            for table in _TOUCHED_TABLES
                        }

                    return await connection.run_sync(inspect_schema)
            finally:
                await snapshot_engine.dispose()

        expected_previous_schema = await schema_snapshot(baseline_url)
        commission_columns = dict(
            (name, nullable)
            for name, _type, nullable, _default
            in expected_previous_schema["commissions"]["columns"]
        )
        assert commission_columns["match_id"] is False

        upgraded = _alembic("upgrade", "head")
        assert upgraded.returncode == 0, upgraded.stderr
        async with engine.begin() as connection:
            await connection.execute(
                text(
                    "INSERT INTO inventory_items "
                    "(fuel_type, current_stock_mt, incoming_stock_mt, reserved_stock_mt) "
                    "VALUES ('Biomethane', 0, 0, 0)"
                )
            )

        refused = _alembic("downgrade", _PREVIOUS_REVISION)
        assert refused.returncode != 0
        refusal = refused.stdout + refused.stderr
        assert "Biomethane requires the widened schema" in refusal

        async with engine.begin() as connection:
            await connection.execute(
                text("DELETE FROM inventory_items WHERE fuel_type = 'Biomethane'")
            )

        downgraded = _alembic("downgrade", _PREVIOUS_REVISION)
        assert downgraded.returncode == 0, downgraded.stderr
        assert await schema_snapshot(analytics_pg_url) == expected_previous_schema
    finally:
        await engine.dispose()
        # Preserve the session database at head even if an intermediate
        # assertion fails, so later correctness tests remain meaningful.
        _alembic("upgrade", "head")
        _admin_sql(f"DROP DATABASE IF EXISTS {baseline_database} WITH (FORCE)")

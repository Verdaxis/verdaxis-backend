"""Executable Alembic drift proofs for market-owned PostgreSQL objects."""
from __future__ import annotations

import asyncio
import os
import subprocess
import sys
from pathlib import Path

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine


_BACKEND_ROOT = Path(__file__).resolve().parents[2]


def _alembic_check(database_url: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "alembic", "check"],
        cwd=_BACKEND_ROOT,
        env={
            "PATH": os.environ.get("PATH", ""),
            "DATABASE_URL": database_url,
            "MIGRATOR_DATABASE_URL": database_url,
            "ENVIRONMENT": "test",
            "RELEASE_SHA": "test",
            "JWT_SECRET": "test-secret-key-that-is-at-least-32-characters-long",
            "BACKEND_CORS_ORIGINS": "[]",
        },
        text=True,
        capture_output=True,
        check=False,
    )


async def _execute_ddl(database_url: str, statement: str) -> None:
    engine = create_async_engine(database_url)
    try:
        async with engine.begin() as connection:
            await connection.execute(text(statement))
    finally:
        await engine.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("mutation", "repair", "evidence"),
    (
        (
            "ALTER TABLE trades ALTER COLUMN quantity_mt TYPE NUMERIC(12,2)",
            "ALTER TABLE trades ALTER COLUMN quantity_mt TYPE NUMERIC",
            "quantity_mt",
        ),
        (
            "ALTER TABLE orderbook_orders "
            "DROP CONSTRAINT fk_orderbook_orders_inventory_item_id",
            "ALTER TABLE orderbook_orders ADD CONSTRAINT "
            "fk_orderbook_orders_inventory_item_id FOREIGN KEY (inventory_item_id) "
            "REFERENCES inventory_items(id) ON DELETE RESTRICT",
            "inventory_item_id",
        ),
        (
            "DROP INDEX ix_trades_status_confirmed_at",
            "CREATE INDEX ix_trades_status_confirmed_at "
            "ON trades (status, confirmed_at)",
            "ix_trades_status_confirmed_at",
        ),
    ),
)
async def test_alembic_check_detects_exact_market_schema_drift(
    market_pg_url: str,
    mutation: str,
    repair: str,
    evidence: str,
):
    await _execute_ddl(market_pg_url, mutation)
    try:
        drift = await asyncio.to_thread(_alembic_check, market_pg_url)
        output = f"{drift.stdout}\n{drift.stderr}".lower()
        assert drift.returncode != 0, output
        assert evidence.lower() in output
    finally:
        await _execute_ddl(market_pg_url, repair)

    clean = await asyncio.to_thread(_alembic_check, market_pg_url)
    assert clean.returncode == 0, f"{clean.stdout}\n{clean.stderr}"
    assert "No new upgrade operations detected" in clean.stdout

"""Real PG17 coverage for the runtime metadata downgrade policy."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine


_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_PREVIOUS_REVISION = "pa_20260715_analytics_facts"


def _alembic(*arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "alembic", *arguments],
        cwd=_BACKEND_ROOT,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.asyncio
async def test_biomethane_refuses_narrowing_then_remediated_roundtrip_succeeds(
    analytics_pg_url,
):
    engine = create_async_engine(analytics_pg_url)
    try:
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
        upgraded = _alembic("upgrade", "head")
        assert upgraded.returncode == 0, upgraded.stderr
    finally:
        await engine.dispose()
        # Preserve the session database at head even if an intermediate
        # assertion fails, so later correctness tests remain meaningful.
        _alembic("upgrade", "head")

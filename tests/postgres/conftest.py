"""Fixtures for the PostgreSQL Product Analytics correctness suite.

The disposable database comes from PRODUCT_ANALYTICS_TEST_DATABASE_URL
(exported by scripts/run_product_analytics_postgres_tests.sh or the CI
service). The fixture refuses any database whose name does not end in
``_analytics_test`` so a mistyped URL can never touch a real database.
Alembic migrations run once per session; involved tables are truncated
before every test.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlsplit

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

_URL_ENV = "PRODUCT_ANALYTICS_TEST_DATABASE_URL"
_BACKEND_ROOT = Path(__file__).resolve().parents[2]

_TRUNCATE_TABLES = (
    "commissions",
    "orders",
    "public_listings",
    "trades",
    "orderbook_orders",
    "users",
    "organizations",
    "products",
    "delivery_points",
)


def _validated_url() -> str:
    url = os.environ.get(_URL_ENV, "").strip()
    if not url:
        pytest.skip(
            f"{_URL_ENV} is not set; run scripts/run_product_analytics_postgres_tests.sh"
        )
    database = urlsplit(url).path.lstrip("/")
    if not database.endswith("_analytics_test"):
        raise RuntimeError(
            f"refusing to run against database {database!r}: "
            "the name must end with _analytics_test"
        )
    return url


@pytest.fixture(scope="session")
def analytics_pg_url() -> str:
    url = _validated_url()
    subprocess.run(
        [sys.executable, "-m", "alembic", "upgrade", "head"],
        check=True,
        cwd=_BACKEND_ROOT,
        env={**os.environ, "DATABASE_URL": url},
    )
    return url


@pytest.fixture
async def pg_session(analytics_pg_url):
    engine = create_async_engine(analytics_pg_url, echo=False)
    try:
        async with engine.begin() as conn:
            await conn.execute(
                text(
                    "TRUNCATE TABLE "
                    + ", ".join(_TRUNCATE_TABLES)
                    + " RESTART IDENTITY CASCADE"
                )
            )
        factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
        async with factory() as session:
            yield engine, session
    finally:
        await engine.dispose()

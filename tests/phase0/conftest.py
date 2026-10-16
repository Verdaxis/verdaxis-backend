"""Synthetic-only connections, supplied by the container-owning runner."""
import os
from pathlib import Path

import pytest
import pytest_asyncio
from sqlalchemy.ext.asyncio import create_async_engine


@pytest_asyncio.fixture
async def source_connection():
    port = os.environ.get("PHASE0_PORT", "")
    if not port.isdigit() or not 1024 < int(port) < 65536:
        pytest.fail("Run through scripts/run_phase0_postgres_tests.sh; no external URL is accepted")
    password = os.environ["PHASE0_PASSWORD"]
    engine = create_async_engine(f"postgresql+asyncpg://phase0_owner:{password}@127.0.0.1:{port}/phase0_prod_test")
    try:
        async with engine.connect() as connection:
            yield connection
    finally:
        await engine.dispose()


def pytest_sessionfinish(session, exitstatus):
    reporter = session.config.pluginmanager.get_plugin("terminalreporter")
    if session.testscollected == 0 or (reporter and reporter.stats.get("skipped")):
        session.exitstatus = 1

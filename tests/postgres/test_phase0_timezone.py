"""Phase 0 Task 6 -- canonical database sessions must run in UTC.

The policy is ``deploy/phase0/set_timezone.sql``: the database default and the
role-in-database override for the app, migrator and backup roles all become
``UTC``. These proofs run only against the disposable container owned by
``scripts/run_phase0_postgres_tests.sh``.

The two session-sensitive day buckets stay in the code. ``price_discovery.py``
buckets with ``func.date(confirmed_at)`` and ``admin_analytics.py`` buckets with
``cast(created_at, Date)``; both convert a ``timestamptz`` through the session
zone. ``product_analytics.utc_date_bucket`` and every FAME certificate date use
an explicit zone, so they must not move. A fresh connection is required: a
session resolves its zone when it connects, so a database default never changes
an already-open session.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from datetime import UTC, date, datetime
from pathlib import Path
from zoneinfo import ZoneInfo

import pytest
from sqlalchemy import (
    Column,
    Date,
    DateTime,
    Integer,
    MetaData,
    Table,
    cast,
    extract,
    func,
    select,
    text,
)
from sqlalchemy.ext.asyncio import create_async_engine

from app.services import product_analytics
from app.services.fame_rfq import delivery_deadline

ROOT = Path(__file__).resolve().parents[2]
POLICY_SQL = ROOT / "deploy" / "phase0" / "set_timezone.sql"

DATABASE = "phase0_tz_target"
APP_ROLE = "phase0_tz_app"
MIGRATOR_ROLE = "phase0_tz_migrator"
BACKUP_ROLE = "phase0_tz_backup"

# 2026-10-16 UTC spans two Singapore calendar days: 16:00:00Z is midnight on
# 17 October in Asia/Singapore, and 23:59:59Z is already that morning.
BOUNDARY_INSTANTS = (
    "2026-10-16 15:59:59+00",
    "2026-10-16 16:00:00+00",
    "2026-10-16 23:59:59+00",
)
UTC_DATES = [date(2026, 10, 16)] * 3
SINGAPORE_DATES = [date(2026, 10, 16), date(2026, 10, 17), date(2026, 10, 17)]

_EVENTS = Table(
    "tz_events",
    MetaData(),
    Column("id", Integer, primary_key=True),
    Column("occurred_at", DateTime(timezone=True), nullable=False),
)


def _port() -> str:
    port = os.environ.get("PHASE0_PORT", "")
    if not port.isdigit() or not 1024 < int(port) < 65536:
        pytest.fail("Run through scripts/run_phase0_postgres_tests.sh; no external URL is accepted")
    return port


def _url(role: str) -> str:
    password = os.environ["PHASE0_PASSWORD"]
    return f"postgresql+asyncpg://{role}:{password}@127.0.0.1:{_port()}/{DATABASE}"


def _psql(
    *arguments: str,
    database: str = "postgres",
    expect_success: bool = True,
) -> subprocess.CompletedProcess:
    """Run psql as the container's superuser. Never prints the password."""
    psql = shutil.which("psql")
    assert psql is not None, "psql is required to exercise deploy/phase0/set_timezone.sql"
    environment = {
        **os.environ,
        "PGPASSWORD": os.environ["PHASE0_PASSWORD"],
        "PGCONNECT_TIMEOUT": "5",
    }
    completed = subprocess.run(
        [psql, "-X", "-q", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1",
         "-p", _port(), "-U", "postgres", "-d", database, *arguments],
        capture_output=True,
        text=True,
        env=environment,
        check=False,
    )
    if expect_success:
        assert completed.returncode == 0, completed.stderr
    return completed


def _apply_policy(*, expect_success: bool = True) -> subprocess.CompletedProcess:
    """Apply the shipped policy exactly as the operator command does."""
    return _psql(
        "-v", f"database_name={DATABASE}",
        "-v", f"app_role={APP_ROLE}",
        "-v", f"migrator_role={MIGRATOR_ROLE}",
        "-v", f"backup_role={BACKUP_ROLE}",
        "-f", str(POLICY_SQL),
        expect_success=expect_success,
    )


def _provision() -> None:
    password = os.environ["PHASE0_PASSWORD"]
    _psql("-c", f"DROP DATABASE IF EXISTS {DATABASE}")
    for role in (APP_ROLE, MIGRATOR_ROLE, BACKUP_ROLE):
        _psql("-c", f"DROP ROLE IF EXISTS {role}")
    for role in (APP_ROLE, MIGRATOR_ROLE, BACKUP_ROLE):
        _psql("-c", f"CREATE ROLE {role} LOGIN PASSWORD '{password}'")
    _psql("-c", f"CREATE DATABASE {DATABASE} OWNER {MIGRATOR_ROLE}")
    _psql(
        "-c",
        "CREATE TABLE tz_events (id integer PRIMARY KEY, "
        "occurred_at timestamptz NOT NULL)",
        database=DATABASE,
    )
    values = ", ".join(
        f"({index}, '{instant}')"
        for index, instant in enumerate(BOUNDARY_INSTANTS, start=1)
    )
    _psql(
        "-c", f"INSERT INTO tz_events (id, occurred_at) VALUES {values}",
        database=DATABASE,
    )
    _psql("-c", f"GRANT SELECT ON tz_events TO {APP_ROLE}", database=DATABASE)


async def _session_buckets(connection) -> dict[str, list[date]]:
    """Reproduce the two session-sensitive buckets and the explicit UTC one."""
    price_reference = func.date(_EVENTS.c.occurred_at)  # price_discovery.py:440
    admin_daily = cast(_EVENTS.c.occurred_at, Date)  # admin_analytics.py:406,421
    login_fact_utc = product_analytics.utc_date_bucket(
        _EVENTS.c.occurred_at, "postgresql"
    )
    rows = (
        await connection.execute(
            select(_EVENTS.c.id, price_reference, admin_daily, login_fact_utc)
            .order_by(_EVENTS.c.id)
        )
    ).all()
    return {
        "price_reference": [row[1] for row in rows],
        "admin_daily": [row[2] for row in rows],
        "login_fact_utc": [row[3] for row in rows],
    }


async def _epochs(connection) -> list[float]:
    rows = (
        await connection.execute(
            select(extract("epoch", _EVENTS.c.occurred_at)).order_by(_EVENTS.c.id)
        )
    ).scalars()
    return [float(value) for value in rows]


async def _timezone(role: str, **connect_arguments) -> str:
    engine = create_async_engine(_url(role), **connect_arguments)
    try:
        async with engine.connect() as connection:
            return (await connection.execute(text("SHOW TimeZone"))).scalar_one()
    finally:
        await engine.dispose()


async def test_timezone_reconnect_and_role_override():
    assert POLICY_SQL.is_file(), "deploy/phase0/set_timezone.sql is missing"
    _provision()

    # Pre-change state: the database default is Singapore, matching the old
    # assumption this task removes.
    _psql("-c", f"ALTER DATABASE {DATABASE} SET timezone='Asia/Singapore'")

    stale = create_async_engine(_url(APP_ROLE))
    try:
        async with stale.connect() as connection:
            assert (await connection.execute(text("SHOW TimeZone"))).scalar_one() == "Asia/Singapore"
            before = await _session_buckets(connection)
            before_epochs = await _epochs(connection)
        assert before["price_reference"] == SINGAPORE_DATES
        assert before["admin_daily"] == SINGAPORE_DATES
        assert before["login_fact_utc"] == UTC_DATES

        # An unexplained role-wide override outranks the database default and
        # silently affects every database, so the policy must refuse it.
        _psql("-c", f"ALTER ROLE {APP_ROLE} SET timezone='Asia/Singapore'")
        refused = _apply_policy(expect_success=False)
        assert refused.returncode != 0
        password = os.environ["PHASE0_PASSWORD"]
        assert password not in refused.stdout and password not in refused.stderr
        _psql("-c", f"ALTER ROLE {APP_ROLE} RESET timezone")

        # Apply the shipped policy from a connection to `postgres`.
        _apply_policy()

        # The connection opened before the change keeps Singapore: a database
        # default never rewrites an already-open session.
        async with stale.connect() as connection:
            assert (await connection.execute(text("SHOW TimeZone"))).scalar_one() == "Asia/Singapore"
    finally:
        # Recycling the pool is the runtime action that picks up UTC.
        await stale.dispose()

    # Fresh connections for every role now report UTC.
    assert await _timezone(APP_ROLE) == "UTC"
    assert await _timezone(MIGRATOR_ROLE) == "UTC"
    assert await _timezone(BACKUP_ROLE) == "UTC"

    engine = create_async_engine(_url(APP_ROLE))
    try:
        async with engine.connect() as connection:
            after = await _session_buckets(connection)
            after_epochs = await _epochs(connection)
    finally:
        await engine.dispose()

    # Session-sensitive buckets move to the UTC day boundary; the explicit UTC
    # bucket and the stored instants do not move at all.
    assert after["price_reference"] == UTC_DATES
    assert after["admin_daily"] == UTC_DATES
    assert after["login_fact_utc"] == UTC_DATES
    assert after_epochs == before_epochs

    # A role-wide override added after the policy is still beaten by the
    # role-in-database value, so the managed setting is authoritative.
    _psql("-c", f"ALTER ROLE {APP_ROLE} SET timezone='Asia/Singapore'")
    try:
        assert await _timezone(APP_ROLE) == "UTC"
    finally:
        _psql("-c", f"ALTER ROLE {APP_ROLE} RESET timezone")

    # A connection option is the one source that outranks the policy. The
    # server cannot see it, so tooling and operators must inspect and reject
    # unexplained options before trusting SHOW TimeZone.
    overridden = await _timezone(
        APP_ROLE, connect_args={"server_settings": {"timezone": "Asia/Singapore"}}
    )
    assert overridden == "Asia/Singapore"

    # The Singapore certificate and delivery-date rules are computed in Python
    # with an explicit zone, so the UTC session cannot move them. At the UTC
    # boundary instant the certificate day is already the next Singapore date
    # while the UTC day bucket is still 16 October.
    boundary = datetime(2026, 10, 16, 16, 0, tzinfo=UTC)
    assert boundary.astimezone(ZoneInfo("Asia/Singapore")).date() == date(2026, 10, 17)
    assert boundary.astimezone(UTC).date() == date(2026, 10, 16)
    assert delivery_deadline(date(2026, 10, 16)) == datetime(2026, 10, 16, 16, 0, tzinfo=UTC)

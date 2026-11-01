#!/usr/bin/env python3
"""Run one retained Python market writer against a disposable test database.

The Rust coexistence target starts this helper as a child process and talks to
it over JSON lines:

1. stdin, line 1: one command object (see ``COMMANDS``);
2. stdout ``READY`` once the application is imported and a connection is open;
3. stdin ``GO`` starts the writer;
4. with ``pause`` set, stdout ``HELD <backend pid>`` after the statement that
   matches the pattern has executed (its locks are held), then stdin
   ``RELEASE`` lets the unchanged service function continue (``pause`` names
   one entry of ``STATEMENTS``);
5. stdout ``RESULT <json>`` with ``ok`` and the writer's result or error.

The service functions run unmodified. The only replaced callable is the email
sender, which records calls instead of sending mail. The helper refuses any
database that is not a loopback database whose name ends in ``_test``.
"""

from __future__ import annotations

import asyncio
import importlib.util
import json
import re
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# Run as a script, Python puts only this folder on the path; the application
# package lives at the repository root.
sys.path.insert(0, str(ROOT))


def emit(kind: str, value: object = None) -> None:
    line = kind if value is None else f"{kind} {value}"
    print(line, flush=True)


def expect(word: str) -> None:
    line = sys.stdin.readline().strip()
    if line != word:
        raise RuntimeError(f"driver protocol expected {word}")


def checked_url(raw: str):
    from sqlalchemy.engine import make_url

    url = make_url(raw)
    if url.host != "127.0.0.1" or not (url.database or "").endswith("_test"):
        raise SystemExit("refusing a database that is not a loopback *_test database")
    if url.drivername != "postgresql+asyncpg":
        raise SystemExit("the driver needs a postgresql+asyncpg URL")
    return url


# Named statements a test may pause after or count.
STATEMENTS = {
    "scan_order_locks": r"FOR UPDATE OF orderbook_orders\s*$",
    "delivery_recheck_locks": r"FOR UPDATE OF users, orderbook_orders",
    "advisory_lock": r"pg_advisory_xact_lock",
    "first_order_insert": r"^\s*INSERT INTO orderbook_orders",
}


class StatementWatch:
    """Pause after the n-th matching statement and count other statements."""

    def __init__(self, pause: str | None, pause_count: int, count: str | None):
        self.pause = re.compile(STATEMENTS[pause]) if pause else None
        self.pause_count = pause_count
        self.count = re.compile(STATEMENTS[count]) if count else None
        self.seen = 0
        self.paused = False
        self.counted = {"before_pause": 0, "after_pause": 0}

    def __call__(self, conn, cursor, statement, parameters, context, executemany):
        if self.count is not None and self.count.search(statement):
            self.counted["after_pause" if self.paused else "before_pause"] += 1
        if self.pause is None or self.paused or not self.pause.search(statement):
            return
        self.seen += 1
        if self.seen < self.pause_count:
            return
        self.paused = True
        pid = conn.connection.driver_connection.get_server_pid()
        # Blocking here keeps this backend's transaction, and every lock it
        # already holds, open until the Rust test releases it.
        emit("HELD", pid)
        expect("RELEASE")


def build_engine(url, application_name: str):
    from app.config import settings
    from app.database import engine_options
    from sqlalchemy.ext.asyncio import create_async_engine

    options = dict(engine_options(settings))
    connect_args = dict(options.get("connect_args", {}))
    server_settings = dict(connect_args.get("server_settings", {}))
    server_settings["application_name"] = application_name
    connect_args["server_settings"] = server_settings
    options["connect_args"] = connect_args
    return create_async_engine(url, **options)


def load_demo_cli():
    path = ROOT / "scripts" / "run_demo_activity.py"
    spec = importlib.util.spec_from_file_location("residual_run_demo_activity", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def instant(value: str | None) -> datetime | None:
    return None if value is None else datetime.fromisoformat(value).astimezone(UTC)


async def reminder_scan(command, engine, factory):
    from app.services.order_expiry_reminders import remind_expiring_orders

    async with factory() as db:
        created = await remind_expiring_orders(db, now=instant(command.get("now")))
        await db.commit()
    return {"created": created}


async def reminder_delivery(command, engine, factory):
    import app.services.order_expiry_reminders as reminders

    calls = []
    accepts = bool(command.get("sender_accepts", True))

    async def recording_sender(payload, notice_id):
        calls.append({"notice_id": str(notice_id), "to": payload.get("to")})
        return accepts

    reminders.send_order_expiry_email = recording_sender
    async with factory() as db:
        sent = await reminders.deliver_expiry_reminder_emails(db, now=instant(command.get("now")))
        await db.commit()
    return {"sent": sent, "sender_calls": calls}


async def demo_find_ticks(command, engine, factory):
    """Recent activity ticks whose seeded slice is the requested product and port."""
    from app.services import demo_activity
    from app.services.availability_windows import availability_window_expiry

    args = command["args"]
    current = datetime.now(UTC)
    base = demo_activity._activity_tick(current)
    found = []
    for step in range(1, 20000):
        reference = base - timedelta(minutes=5 * step)
        if current - reference > timedelta(days=6):
            break
        demo_activity._RNG.seed(int(reference.timestamp() // 300))
        product, port, window = demo_activity._activity_slice(reference)
        if (product, port) != (args["product"], args["port"]):
            continue
        if availability_window_expiry(window, observed_at=reference) <= current + timedelta(hours=6):
            continue
        found.append({"now": reference.isoformat(), "window": window})
        if len(found) == int(args["count"]):
            break
    return {"ticks": found}


async def demo_tick(command, engine, factory):
    from app.services.demo_activity import (
        ensure_demo_activity_organizations,
        generate_demo_market_activity,
    )

    cli = load_demo_cli()
    async with factory() as db:
        await ensure_demo_activity_organizations(db)
        await db.commit()
        result = await generate_demo_market_activity(db, now=instant(command.get("now")))
        cli._validate_activity_result(result)
        await cli._enqueue_demo_refresh(
            db,
            operation="tick",
            changed=bool(result.get("created_orders") or result.get("created_trades")),
        )
        await db.commit()
    return result


async def demo_coverage(command, engine, factory):
    """The coverage step of the demo timer, then its outbox event, as the CLI runs them."""
    from app.seeds import market_seed
    from app.services import demo_activity

    # Fixture size only: the B30 Rotterdam market. Locks, SQL and rows are the
    # unchanged coverage refresh.
    demo_activity.PRICING = {"B30": {"Rotterdam": market_seed.PRICING["B30"]["Rotterdam"]}}
    cli = load_demo_cli()
    async with factory() as db:
        coverage = await demo_activity.ensure_demo_market_coverage(db, now=instant(command.get("now")))
        await cli._enqueue_demo_refresh(
            db,
            operation="coverage",
            changed=any(
                coverage.get(key, 0)
                for key in ("coverage_created", "coverage_refreshed", "coverage_expired", "legacy_activity_expired")
            ),
        )
        await db.commit()
    return coverage


async def demo_prune(command, engine, factory):
    from app.services.demo_activity import prune_demo_activity

    cli = load_demo_cli()
    async with factory() as db:
        result = await prune_demo_activity(db, now=instant(command.get("now")))
        await cli._enqueue_demo_refresh(
            db,
            operation="prune",
            changed=any(result.get(key, 0) for key in ("trades_pruned", "orders_pruned")),
        )
        await db.commit()
    return result


async def quarantine(command, engine, factory):
    from app.services.market_quarantine import OperatorContext, quarantine_orders

    context = OperatorContext(
        environment="test",
        database_name=engine.url.database,
        operator="coexistence-check",
        reason="Disposable coexistence check",
        reference="coexistence-check",
    )
    async with engine.begin() as connection:
        reports = await quarantine_orders(
            connection, command["args"]["order_ids"], context=context, apply=True
        )
    return {"reports": len(reports)}


async def seed_reset(command, engine, factory):
    from app.seeds.market_seed import reset_demo_market_data

    async with factory() as db:
        await reset_demo_market_data(db)
        await db.commit()
    return {}


async def seed_market(command, engine, factory):
    from app.seeds import market_seed
    from app.seeds.catalog_seed import seed_catalog
    from app.seeds.safety import resolve_seed_target
    from app.services import demo_activity

    url = engine.url
    resolve_seed_target(
        {
            "SEED_DATABASE_URL": url.render_as_string(hide_password=False),
            "ALLOW_SEED_MUTATIONS": "I_UNDERSTAND_SEED_MUTATIONS",
            "SEED_RUNTIME_ENV": "disposable",
            "SEED_TARGET_DATABASE": url.database,
        }
    )
    # Fixture size only, as the project's own seed test does: two markets keep
    # the run short. Locks, SQL and row construction are the unchanged seed.
    pricing = {
        "Bio Methanol": {"Singapore": market_seed.PRICING["Bio Methanol"]["Singapore"]},
        "Bio Ethanol": {"Rotterdam": market_seed.PRICING["Bio Ethanol"]["Rotterdam"]},
    }
    market_seed.PRICING = pricing
    demo_activity.PRICING = pricing
    async with factory() as db:
        await seed_catalog(db)
        await market_seed.seed_market_data(db)
    return {}


class RecordingBus:
    """The dispatcher's event bus, recording every publish instead of serving SSE."""

    def __init__(self) -> None:
        self.frames: list[dict] = []
        self.released_at: float | None = None

    async def publish(self, channel, event_type, payload, seq=None):
        loop = asyncio.get_running_loop()
        elapsed = None if self.released_at is None else loop.time() - self.released_at
        self.frames.append(
            {"channel": channel, "event_type": event_type, "payload": payload, "seq": seq, "after_release_s": elapsed}
        )


async def dispatch(command, engine, factory):
    """Python's unchanged dispatcher: sequencer, hub fan-out, then org replay.

    After the dispatcher holds the sequencer lock and is idle, ``HELD <listener
    pid>`` lets the Rust test commit its writes; ``RELEASE`` then starts the
    wait for the expected private frames. Rust writers send no wake, so the
    frames arrive through the dispatcher's own poll.
    """
    from app.models.market_event import MarketEventOutbox
    from app.services.market_event_dispatch import (
        MarketEventDispatcher,
        fetch_events_for_org,
    )

    args = command["args"]
    loop = asyncio.get_running_loop()
    bus = RecordingBus()
    dispatcher = MarketEventDispatcher(engine, bus)
    await dispatcher.start()
    try:
        deadline = loop.time() + 20
        while not dispatcher._is_leader:
            if loop.time() > deadline:
                raise RuntimeError("the Python dispatcher did not take the sequencer lock")
            await asyncio.sleep(0.02)
        emit("HELD", dispatcher._listener_conn.get_server_pid())
        expect("RELEASE")
        bus.released_at = loop.time()
        expected = int(args["expect_private"])
        deadline = loop.time() + 20
        while sum(1 for frame in bus.frames if frame["seq"] is not None) < expected:
            if loop.time() > deadline:
                break
            await asyncio.sleep(0.02)
    finally:
        await dispatcher.stop()
    replay = {}
    async with factory() as db:
        for org in args["orgs"]:
            rows = await fetch_events_for_org(db, org, after_seq=int(args["after_seq"]))
            replay[org] = [
                {
                    "seq": row.stream_seq,
                    "event_type": row.event_type,
                    "payload": row.payload,
                    "participants": [str(value) for value in row.participant_org_ids],
                    "public": row.public_market_invalidation,
                    "fields": list(type(row).__slots__),
                }
                for row in rows
            ]
    return {
        "frames": bus.frames,
        "replay": replay,
        "mapped_columns": sorted(MarketEventOutbox.__table__.columns.keys()),
    }


COMMANDS = {
    "dispatch": dispatch,
    "reminder_scan": reminder_scan,
    "reminder_delivery": reminder_delivery,
    "demo_find_ticks": demo_find_ticks,
    "demo_tick": demo_tick,
    "demo_coverage": demo_coverage,
    "demo_prune": demo_prune,
    "quarantine": quarantine,
    "seed_reset": seed_reset,
    "seed_market": seed_market,
}


def describe(error: BaseException) -> dict:
    from app.services.db_errors import database_sqlstate
    from fastapi import HTTPException

    value = {"type": type(error).__name__, "message": str(error)[:500]}
    if isinstance(error, HTTPException):
        value["status_code"] = error.status_code
    sqlstate = database_sqlstate(error)
    if sqlstate:
        value["sqlstate"] = sqlstate
    return value


async def run(command: dict) -> dict:
    from sqlalchemy import event, text
    from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

    url = checked_url(command["database_url"])
    engine = build_engine(url, command.get("application_name", "phase8_residual_py"))
    watch = StatementWatch(command.get("pause"), int(command.get("pause_count", 1)), command.get("count"))
    event.listen(engine.sync_engine, "after_cursor_execute", watch)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    try:
        async with engine.connect() as connection:
            await connection.execute(text("SELECT 1"))
        emit("READY")
        expect("GO")
        try:
            result = await COMMANDS[command["op"]](command, engine, factory)
            outcome = {"ok": True, "result": result}
        except Exception as error:  # reported to the Rust test, which asserts it
            outcome = {"ok": False, "error": describe(error)}
        outcome["paused"] = watch.paused
        outcome["counted"] = watch.counted
        return outcome
    finally:
        await engine.dispose()


def main() -> None:
    command = json.loads(sys.stdin.readline())
    if command.get("op") not in COMMANDS:
        raise SystemExit("unknown driver command")
    outcome = asyncio.run(run(command))
    emit("RESULT", json.dumps(outcome, default=str, sort_keys=True))


if __name__ == "__main__":
    main()

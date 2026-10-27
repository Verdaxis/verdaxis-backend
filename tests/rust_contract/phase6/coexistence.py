"""Bounded controls for real Python producers and the Rust election fixture."""
from __future__ import annotations

import asyncio
import json
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


class RustActor:
    def __init__(self, barrier=False):
        self.process = subprocess.Popen(
            [str(ROOT / "rs/target/debug/examples/streams_fixture")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
            env={**os.environ, "STREAM_FIXTURE_APP_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg", "postgresql"),
                 "STREAM_FIXTURE_BARRIER": "assigned" if barrier else ""},
        )

    async def receive(self):
        line = await asyncio.wait_for(asyncio.to_thread(self.process.stdout.readline), 15)
        assert line, "Rust fixture ended before its barrier"
        return json.loads(line)

    def send(self, command):
        self.process.stdin.write(command + "\n")
        self.process.stdin.flush()

    async def close(self):
        if self.process.poll() is None:
            self.send("stop")
            await asyncio.wait_for(asyncio.to_thread(self.process.wait), 15)
        assert self.process.returncode == 0, self.process.stderr.read()


async def python_worker(connection):
    """A process owns its production dispatcher and its SQLAlchemy producer."""
    from uuid import UUID

    from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

    from app.services.market_event_dispatch import MarketEventDispatcher
    from app.services.market_events import (
        enqueue_market_events,
        participant_market_event,
    )

    engine = create_async_engine(os.environ["DATABASE_URL"], pool_size=2, max_overflow=1)
    dispatcher = MarketEventDispatcher(engine, None)
    await dispatcher._connect_listener()
    connection.send({"ready": os.getpid(), "pid": dispatcher._listener_conn.get_server_pid()})
    pending = None
    try:
        while True:
            command = await asyncio.to_thread(connection.recv)
            operation = command["op"]
            if operation == "stop":
                break
            if operation == "elect":
                connection.send({"leader": await dispatcher._try_acquire_leadership()})
            elif operation == "assign":
                assert await dispatcher._try_acquire_leadership()
                connection.send({"assigned": await dispatcher._assign_pending_sequences()})
            elif operation == "release":
                if dispatcher._is_leader:
                    await dispatcher._listener_conn.execute("SELECT pg_advisory_unlock($1)", 7264537260226625)
                    dispatcher._is_leader = False
                connection.send({"released": True})
            elif operation == "produce":
                pending = async_sessionmaker(engine)()
                event = participant_market_event(event_type="trade_created", aggregate_type="trade",
                    aggregate_id=command["aggregate"], participant_org_ids=[UUID(command["org"])],
                    payload={"trade_id": command["aggregate"], "quantity_mt":"10"})
                ids = await enqueue_market_events(pending, [event])
                connection.send({"pending": [str(value) for value in ids]})
            elif operation == "commit":
                await pending.commit()
                await pending.close()
                pending = None
                connection.send({"committed": True})
            else:
                raise AssertionError("unknown worker operation")
    finally:
        if pending is not None:
            await pending.rollback()
            await pending.close()
        await dispatcher._listener_conn.close()
        await engine.dispose()
        connection.close()


def worker_main(connection):
    asyncio.run(python_worker(connection))


async def request(connection, command):
    connection.send(command)
    assert await asyncio.to_thread(connection.poll, 15), "Python worker missed its barrier"
    return connection.recv()

"""Four Python producers share one disposable outbox with a Rust sequencer."""
from __future__ import annotations

import asyncio
import multiprocessing
import os
import subprocess
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.test_phase6_http import _require_app_role
from tests.rust_contract.phase6.coexistence import RustActor, request, worker_main

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def stream_fixture_binary():
    result = subprocess.run(["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked",
        "-p", "verdaxis-read", "--features", "contract-tests", "--example", "streams_fixture"],
        cwd=ROOT, capture_output=True, text=True, timeout=1800, check=False)
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.asyncio
async def test_four_python_workers_and_rust_sequence_committed_rows(analytics_pg_url, stream_fixture_binary):
    await _require_app_role(os.environ["DATABASE_URL"])
    context = multiprocessing.get_context("spawn")
    workers = []
    actor = RustActor()
    owner = create_async_engine(analytics_pg_url)
    try:
        ready = await actor.receive()
        assert ready["stage"] == "ready"
        for _ in range(4):
            parent, child = context.Pipe()
            process = context.Process(target=worker_main, args=(child,))
            process.start()
            assert await asyncio.to_thread(parent.poll, 15)
            worker = parent.recv()
            workers.append((process, parent, worker))
        assert len({worker[2]["ready"] for worker in workers}) == 4
        assert len({worker[2]["pid"] for worker in workers}) == 4
        org = str(uuid4())
        pending_ids = []
        for _, pipe, _ in workers:
            response = await request(pipe, {"op":"produce", "org":org, "aggregate":str(uuid4())})
            pending_ids.extend(response["pending"])
        # Three fast commits are visible; the fourth producer is still at its barrier.
        for _, pipe, _ in workers[1:]:
            await request(pipe, {"op":"commit"})
        leader = workers[1][1]
        assert (await request(leader, {"op":"elect"}))["leader"]
        for _, pipe, _ in workers[2:]:
            assert not (await request(pipe, {"op":"elect"}))["leader"]
        actor.send("assign")
        assert (await actor.receive())["outcome"] == "Ok(NotElected)"
        assert (await request(leader, {"op":"assign"}))["assigned"] == 3
        async with owner.connect() as connection:
            rows = (await connection.execute(text("SELECT id::text,stream_seq,delivery_attempts FROM market_event_outbox WHERE participant_org_ids::jsonb ? :org ORDER BY stream_seq"), {"org":org})).all()
        assert {row[0] for row in rows} == set(pending_ids[1:])
        assert all(row[2] == 1 for row in rows)
        high_water = max(row[1] for row in rows)
        await request(leader, {"op":"release"})
        await request(workers[0][1], {"op":"commit"})
        actor.send("assign")
        assert "Committed { assigned: 1 }" in (await actor.receive())["outcome"]
        async with owner.connect() as connection:
            rows = (await connection.execute(text("SELECT id::text,stream_seq,delivery_attempts FROM market_event_outbox WHERE participant_org_ids::jsonb ? :org ORDER BY stream_seq"), {"org":org})).all()
        assert {row[0] for row in rows} == set(pending_ids)
        assert rows[-1][0] == pending_ids[0] and rows[-1][1] > high_water
        assert len({row[1] for row in rows}) == 4
        assert all(row[2] == 1 for row in rows)
        print({"worker_processes":[worker[2]["ready"] for worker in workers],
               "committed_rows":[tuple(row) for row in rows], "leaders":["python","rust"]})
    finally:
        for process, pipe, _ in workers:
            if process.is_alive():
                pipe.send({"op":"stop"})
            await asyncio.to_thread(process.join, 15)
            assert not process.is_alive(), "Python worker did not stop"
            assert process.exitcode == 0
            pipe.close()
        await actor.close()
        await owner.dispose()

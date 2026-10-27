"""Barrier-controlled assignment loss on the disposable shared database."""
from __future__ import annotations

import os
import subprocess
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.rust_contract.phase6.coexistence import RustActor


@pytest.mark.asyncio
async def test_rust_assignment_connection_loss_is_atomic(analytics_pg_url):
    coexistence_fixture.stream_fixture_binary.__wrapped__()
    owner = create_async_engine(analytics_pg_url)
    admin = create_async_engine(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
    org, event = str(uuid4()), uuid4()
    async with owner.begin() as connection:
        await connection.execute(text("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload) VALUES (:id,'trade_created','trade',:aggregate,CAST(:participants AS json),CAST(:payload AS json))"),
            {"id":event,"aggregate":str(uuid4()),"participants":f'["{org}"]',"payload":'{"quantity_mt":"10"}'})
    actor = RustActor(barrier=True)
    successor = None
    try:
        ready = await actor.receive()
        actor.send("assign")
        assigned = await actor.receive()
        assert assigned == {"stage":"assigned", "pid":ready["pid"]}
        async with owner.connect() as connection:
            assert (await connection.execute(text("SELECT stream_seq FROM market_event_outbox WHERE id=:id"), {"id":event})).scalar_one() is None
        async with admin.begin() as connection:
            assert (await connection.execute(text("SELECT pg_terminate_backend(:pid)"), {"pid":ready["pid"]})).scalar_one()
        actor.send("continue")
        assert (await actor.receive())["outcome"].startswith("Err(")
        successor = RustActor()
        await successor.receive()
        successor.send("assign")
        assert "Committed { assigned: 1 }" in (await successor.receive())["outcome"]
        async with owner.connect() as connection:
            row = (await connection.execute(text("SELECT id::text,stream_seq,delivery_attempts FROM market_event_outbox WHERE id=:id"), {"id":event})).one()
        assert row[0] == str(event) and row[1] > 0 and row[2] == 1
        print({"row":tuple(row),"terminated_backend":ready["pid"],"outcome":"atomic_successor_assignment"})
    finally:
        await actor.close()
        if successor is not None:
            await successor.close()
        await owner.dispose()
        await admin.dispose()


@pytest.mark.asyncio
async def test_three_runtime_restart_cycles(analytics_pg_url):
    result = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
         "--features", "contract-tests", "--test", "streams_coexistence", "--", "--ignored", "--nocapture"],
        cwd=Path(__file__).resolve().parents[2],
        env={**os.environ, "STREAM_RUNTIME_APP_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg", "postgresql"),
             "STREAM_RUNTIME_ADMIN_URL": os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"].replace("postgresql+asyncpg", "postgresql")},
        capture_output=True, text=True, timeout=300, check=False,
    )
    print(result.stdout)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "1 passed; 0 failed; 0 ignored" in result.stdout
    for cycle in range(1, 4):
        assert f"cycle={cycle} " in result.stdout

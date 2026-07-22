"""Unit coverage for the durable shared SSE dispatch building blocks.

The PostgreSQL-only pieces (single-leader sequencing, LISTEN/NOTIFY wake,
real 4-worker cross-process delivery) are proven in
tests/postgres/test_market_event_dispatch.py; this module covers the
dialect-independent contracts on the SQLite harness.
"""
from __future__ import annotations

from pathlib import Path
from uuid import uuid4

import pytest
from fastapi import FastAPI, HTTPException
from httpx import ASGITransport, AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.model_base import Base
from app.models.market_event import MarketEventOutbox
from app.routers.stream import _format_market_event, _parse_last_event_id
from app.services.event_bus import EventBus
from app.services.market_event_dispatch import (
    MarketEventDispatcher,
    fetch_events_for_org,
    stream_channel_for_org,
)
from app.services.market_events import (
    commit_market_events,
    participant_market_event,
)

pytestmark = pytest.mark.asyncio


@pytest.fixture
async def sqlite_env():
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    async with engine.begin() as conn:
        await conn.run_sync(
            Base.metadata.create_all, tables=[MarketEventOutbox.__table__]
        )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield engine, session
    await engine.dispose()


async def _seed_sequenced_events(session, org_a, org_b):
    """Three sequenced events (A, A+B, B) plus one unsequenced row."""
    specs = [
        ("a_only", [str(org_a)], 11),
        ("shared", [str(org_a), str(org_b)], 12),
        ("b_only", [str(org_b)], 13),
        ("pending", [str(org_a)], None),
    ]
    for event_type, participants, seq in specs:
        session.add(
            MarketEventOutbox(
                event_type=event_type,
                aggregate_type="order",
                aggregate_id=str(uuid4()),
                participant_org_ids=participants,
                payload={"marker": event_type},
                stream_seq=seq,
            )
        )
    await session.commit()


async def test_replay_is_org_scoped_ordered_and_excludes_unsequenced(sqlite_env):
    _, session = sqlite_env
    org_a, org_b = uuid4(), uuid4()
    await _seed_sequenced_events(session, org_a, org_b)

    events_a = await fetch_events_for_org(session, org_a, after_seq=0)
    assert [event.event_type for event in events_a] == ["a_only", "shared"]
    assert [event.stream_seq for event in events_a] == [11, 12]

    events_b = await fetch_events_for_org(session, org_b, after_seq=0)
    assert [event.event_type for event in events_b] == ["shared", "b_only"]

    # The cursor is exclusive: replaying from the last seen id yields only
    # strictly newer events, never a duplicate of the cursor row.
    assert [
        event.stream_seq
        for event in await fetch_events_for_org(session, org_a, after_seq=11)
    ] == [12]
    assert await fetch_events_for_org(session, org_a, after_seq=12) == []


async def test_enqueue_on_sqlite_commits_without_postgresql_notify(sqlite_env):
    _, session = sqlite_env
    org = uuid4()
    ids = await commit_market_events(
        session,
        [
            participant_market_event(
                event_type="order_created",
                aggregate_type="order",
                aggregate_id=uuid4(),
                participant_org_ids=[org],
                payload={"ok": True},
            )
        ],
    )
    assert len(ids) == 1
    rows = await fetch_events_for_org(session, org, after_seq=0)
    assert rows == []  # not sequenced yet, therefore not visible on streams


async def test_dispatcher_is_disabled_on_non_postgresql_engines(sqlite_env):
    engine, _ = sqlite_env
    dispatcher = MarketEventDispatcher(engine, EventBus())
    assert dispatcher.enabled is False
    await dispatcher.start()
    assert dispatcher._tasks == []
    await dispatcher.stop()


async def test_event_bus_carries_the_durable_sequence():
    bus = EventBus()
    channel = stream_channel_for_org(uuid4())
    queue = bus.subscribe(channel)
    await bus.publish(channel, "order_created", {"x": 1}, seq=7)
    await bus.publish(channel, "local_only", {"y": 2})
    sequenced = queue.get_nowait()
    local = queue.get_nowait()
    assert sequenced["seq"] == 7
    assert "seq" not in local


def test_sse_frame_format_carries_id_only_for_sequenced_events():
    framed = _format_market_event(42, "order_created", {"a": 1})
    assert framed == 'id: 42\nevent: order_created\ndata: {"a": 1}\n\n'
    unsequenced = _format_market_event(None, "notice", {"a": 1})
    assert unsequenced.startswith("event: notice\n")
    assert "id:" not in unsequenced


class _FakeRequest:
    def __init__(self, header_value: str | None = None):
        self.headers = (
            {} if header_value is None else {"last-event-id": header_value}
        )


def test_last_event_id_header_wins_and_garbage_is_rejected():
    assert _parse_last_event_id(_FakeRequest(), None) is None
    assert _parse_last_event_id(_FakeRequest(), "5") == 5
    assert _parse_last_event_id(_FakeRequest("9"), "5") == 9
    # An empty header is treated as absent, matching EventSource semantics.
    assert _parse_last_event_id(_FakeRequest(""), None) is None
    for garbage in ("-1", "abc", "1.5"):
        with pytest.raises(HTTPException) as excinfo:
            _parse_last_event_id(_FakeRequest(garbage), None)
        assert excinfo.value.status_code == 400


def test_stream_migration_refuses_downgrade_and_extends_the_linear_chain():
    source = Path("alembic/versions/sse_20260720_market_event_stream.py").read_text()
    assert 'down_revision = "mi_20260720_market_integrity"' in source
    assert "raise RuntimeError" in source
    checkpoints = Path("deploy/migration-checkpoints.tsv").read_text().splitlines()
    assert "mi_20260720_market_integrity\tsse_20260720_market_event_stream" in checkpoints
    assert (
        "sse_20260720_market_event_stream\tsse_20260720_market_event_stream"
        in checkpoints
    )


async def test_cross_tenant_channels_never_share_a_queue():
    bus = EventBus()
    org_x, org_y = uuid4(), uuid4()
    queue_x = bus.subscribe(stream_channel_for_org(org_x))
    queue_y = bus.subscribe(stream_channel_for_org(org_y))
    await bus.publish(stream_channel_for_org(org_x), "x_event", {"org": str(org_x)}, seq=1)
    assert queue_x.get_nowait()["event"] == "x_event"
    assert queue_y.empty()


async def test_stream_trades_rejects_invalid_cursor_before_token_checks():
    from app.routers.stream import router

    app = FastAPI()
    app.include_router(router, prefix="/api")
    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        response = await client.get(
            "/api/stream/trades",
            params={"last_event_id": "not-a-number"},
        )
    assert response.status_code == 400
    assert response.json()["detail"]["code"] == "STREAM_CURSOR_INVALID"

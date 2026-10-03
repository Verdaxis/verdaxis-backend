"""Unit coverage for the durable shared SSE dispatch building blocks.

The PostgreSQL-only pieces (single-leader sequencing, LISTEN/NOTIFY wake,
real 4-worker cross-process delivery) are proven in
tests/postgres/test_market_event_dispatch.py; this module covers the
dialect-independent contracts on the SQLite harness.
"""
from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch
from uuid import uuid4

import pytest
from fastapi import FastAPI, HTTPException
from httpx import ASGITransport, AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.model_base import Base
from app.models.market_event import MarketEventOutbox
from app.routers import stream
from app.routers.stream import _format_market_event, _parse_last_event_id
from app.services.event_bus import EventBus, SubscriberQueue
from app.services.market_event_dispatch import (
    MarketEventDispatcher,
    PUBLIC_INVALIDATION_EVENT,
    PUBLIC_INVALIDATION_PAYLOAD,
    fetch_events_for_org,
    fetch_replay_high_water,
    stream_channel_for_org,
)
from app.services.market_events import (
    PUBLIC_MARKET_INVALIDATION_KEY,
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
    assert [
        event.stream_seq
        for event in await fetch_events_for_org(
            session, org_b, after_seq=0, through_seq=12
        )
    ] == [12]
    assert await fetch_replay_high_water(session) == 13


class _ReplaySession:
    async def __aenter__(self):
        return self

    async def __aexit__(self, *_exc_info):
        return None

    async def rollback(self):
        return None


class _DisconnectAfterReads:
    def __init__(self, allowed_reads: int):
        self.headers = {}
        self._allowed_reads = allowed_reads
        self._reads = 0

    async def is_disconnected(self):
        self._reads += 1
        return self._reads > self._allowed_reads


def _event(seq: int):
    return SimpleNamespace(
        stream_seq=seq,
        event_type="proof_marker",
        payload={"schema_version": 1, "marker": f"event-{seq}"},
    )


async def test_catch_up_pages_to_high_water_then_merges_queued_live_events():
    organization_id = uuid4()
    replay_sequences = list(range(2, 1204, 2))  # 601 authorized rows with holes
    fetch_cursors = []

    async def fetch_page(_db, _org, *, after_seq, through_seq, limit=500):
        assert _org == organization_id
        assert through_seq == replay_sequences[-1]
        fetch_cursors.append(after_seq)
        page = [
            seq
            for seq in replay_sequences
            if after_seq < seq <= through_seq
        ][:limit]
        return [_event(seq) for seq in page]

    queue = SubscriberQueue()
    for seq in (1305, replay_sequences[-1], 1205):
        queue.put_nowait(
            {
                "event": "proof_marker",
                "data": {"schema_version": 1, "marker": f"event-{seq}"},
                "seq": seq,
            }
        )

    with (
        patch.object(stream, "AsyncSessionLocal", side_effect=lambda: _ReplaySession()),
        patch.object(
            stream,
            "fetch_replay_high_water",
            new=AsyncMock(return_value=replay_sequences[-1]),
        ),
        patch.object(stream, "fetch_events_for_org", side_effect=fetch_page),
        patch.object(
            stream,
            "_validate_private_stream_authorization",
            new=AsyncMock(return_value=None),
        ),
        patch.object(stream.time, "time", return_value=0),
    ):
        frames = [
            frame
            async for frame in stream._private_sse_messages(
                _DisconnectAfterReads(allowed_reads=3),
                queue,
                uuid4(),
                organization_id,
                {"exp": 10_000},
                last_event_id=0,
            )
        ]

    delivered_ids = [
        int(frame.split("\n", 1)[0].split(": ", 1)[1])
        for frame in frames
        if frame.startswith("id:")
    ]
    assert delivered_ids == replay_sequences + [1205, 1305]
    assert fetch_cursors == [0, replay_sequences[499]]


async def test_catch_up_expires_and_revalidates_authorization_between_rows():
    organization_id = uuid4()
    queue = SubscriberQueue()

    with (
        patch.object(stream, "AsyncSessionLocal", side_effect=lambda: _ReplaySession()),
        patch.object(stream, "fetch_replay_high_water", new=AsyncMock(return_value=4)),
        patch.object(
            stream,
            "fetch_events_for_org",
            new=AsyncMock(return_value=[_event(2), _event(4)]),
        ),
        patch.object(
            stream,
            "_validate_private_stream_authorization",
            new=AsyncMock(side_effect=[None, None, ValueError("revoked")]),
        ) as validate,
        patch.object(stream.time, "time", side_effect=[0, 16, 32]),
    ):
        frames = [
            frame
            async for frame in stream._private_sse_messages(
                _DisconnectAfterReads(allowed_reads=0),
                queue,
                uuid4(),
                organization_id,
                {"exp": 100},
                last_event_id=0,
            )
        ]

    assert sum(frame.startswith("id:") for frame in frames) == 1
    assert "auth_revoked" in frames[-1]
    assert validate.await_count == 3


async def test_catch_up_stops_at_token_expiry_mid_page():
    organization_id = uuid4()
    queue = SubscriberQueue()

    with (
        patch.object(stream, "AsyncSessionLocal", side_effect=lambda: _ReplaySession()),
        patch.object(stream, "fetch_replay_high_water", new=AsyncMock(return_value=4)),
        patch.object(
            stream,
            "fetch_events_for_org",
            new=AsyncMock(return_value=[_event(2), _event(4)]),
        ),
        patch.object(
            stream,
            "_validate_private_stream_authorization",
            new=AsyncMock(return_value=None),
        ),
        patch.object(stream.time, "time", side_effect=[0, 0, 11]),
    ):
        frames = [
            frame
            async for frame in stream._private_sse_messages(
                _DisconnectAfterReads(allowed_reads=0),
                queue,
                uuid4(),
                organization_id,
                {"exp": 10},
                last_event_id=0,
            )
        ]

    assert sum(frame.startswith("id:") for frame in frames) == 1
    assert "auth_expired" in frames[-1]


async def test_terminal_overflow_reset_closes_the_stream():
    organization_id = uuid4()
    queue = SubscriberQueue()
    for index in range(queue.maxsize):
        queue.put_nowait({"event": "proof_marker", "data": {"index": index}})
    queue.terminate_for_overflow()

    with (
        patch.object(
            stream,
            "_validate_private_stream_authorization",
            new=AsyncMock(return_value=None),
        ),
        patch.object(stream.time, "time", return_value=0),
    ):
        frames = [
            frame
            async for frame in stream._private_sse_messages(
                _DisconnectAfterReads(allowed_reads=1),
                queue,
                uuid4(),
                organization_id,
                {"exp": 100},
                last_event_id=None,
            )
        ]

    assert len(frames) == 2
    assert frames[-1].startswith("event: reset\n")
    assert '"resync_required": true' in frames[-1]


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


async def test_public_marker_is_stripped_and_drain_is_coalesced_without_sequence():
    org = uuid4()
    rows = [
        {
            "id": uuid4(),
            "event_type": "order_created",
            "participant_org_ids": [str(org)],
            "payload": {
                "schema_version": 1,
                "private": "value",
                PUBLIC_MARKET_INVALIDATION_KEY: True,
            },
            "stream_seq": 1,
        },
        {
            "id": uuid4(),
            "event_type": "order_updated",
            "participant_org_ids": [str(org)],
            "payload": {PUBLIC_MARKET_INVALIDATION_KEY: True},
            "stream_seq": 2,
        },
    ]

    class _Mappings:
        def __init__(self, values):
            self._values = values

        def mappings(self):
            return self._values

    class _Connection:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def execute(self, _query):
            nonlocal rows
            values, rows = rows, []
            return _Mappings(values)

    class _Engine:
        def connect(self):
            return _Connection()

    bus = EventBus()
    private_queue = bus.subscribe(stream_channel_for_org(org))
    price_queue = bus.subscribe("prices")
    orderbook_queue = bus.subscribe("orderbook")
    dispatcher = MarketEventDispatcher(_Engine(), bus, batch_size=500)

    await dispatcher._drain_new_events()

    private_events = [private_queue.get_nowait(), private_queue.get_nowait()]
    assert all(
        PUBLIC_MARKET_INVALIDATION_KEY not in item["data"]
        for item in private_events
    )
    assert [item["seq"] for item in private_events] == [1, 2]
    for queue in (price_queue, orderbook_queue):
        public_event = queue.get_nowait()
        assert public_event["event"] == PUBLIC_INVALIDATION_EVENT
        assert public_event["data"] == PUBLIC_INVALIDATION_PAYLOAD
        assert "seq" not in public_event
        assert queue.empty()


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


class _FenceListenerConn:
    """Records assignment statements executed on the lock-holder session."""

    def __init__(self):
        self.fetch_calls: list[tuple[str, tuple]] = []

    def is_closed(self) -> bool:
        return False

    async def fetch(self, sql, *args):
        self.fetch_calls.append((sql, args))
        return [object(), object()]


class _PoolForbiddenEngine:
    """Any pool usage during sequence assignment is a fencing violation."""

    def begin(self):  # pragma: no cover - reaching this IS the failure
        raise AssertionError(
            "sequence assignment must run on the advisory-lock session, "
            "never on a pool connection"
        )

    connect = begin


async def test_sequence_assignment_is_fenced_to_the_lock_holding_connection():
    """Dual-assigner fence: the assignment UPDATE runs on the same

    PostgreSQL session that holds the sequencer advisory lock. If it ran on
    a pool connection, a selectively dropped listener connection would
    release the lock mid-assignment and let a second leader assign
    concurrently, committing sequences out of visibility order and
    permanently skipping events past every hub cursor.
    """
    dispatcher = MarketEventDispatcher(_PoolForbiddenEngine(), bus=None)
    listener = _FenceListenerConn()
    dispatcher._listener_conn = listener

    assigned = await dispatcher._assign_pending_sequences()

    assert assigned == 2
    assert len(listener.fetch_calls) == 1
    sql, args = listener.fetch_calls[0]
    assert "FOR UPDATE SKIP LOCKED" in sql
    assert "nextval('market_event_stream_seq')" in sql
    assert args == (dispatcher._batch_size,)

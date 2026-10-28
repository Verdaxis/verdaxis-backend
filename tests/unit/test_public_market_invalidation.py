"""Public ``prices`` / ``orderbook`` refresh signal contract.

Public ``market_invalidated`` frames are id-less, idempotent "refetch"
signals (docs/market-event-dispatch.md). The outbox ``stream_seq`` is never
exposed on them, so they give no ordering or gap guarantees. This module
covers the hub behavior behind that contract, including that a marker is not
lost when a drain step fails after the cursor moved past it.
"""
from __future__ import annotations

from uuid import uuid4

import pytest
from app.routers import stream
from app.services.event_bus import EventBus
from app.services.market_event_dispatch import (
    PUBLIC_INVALIDATION_EVENT,
    PUBLIC_INVALIDATION_PAYLOAD,
    MarketEventDispatcher,
)
from app.services.market_events import PUBLIC_MARKET_INVALIDATION_KEY

pytestmark = pytest.mark.asyncio


def _row(seq: int, *, public: bool) -> dict:
    payload = {"schema_version": 1, "private": f"value-{seq}"}
    if public:
        payload[PUBLIC_MARKET_INVALIDATION_KEY] = True
    return {
        "id": uuid4(),
        "event_type": "order_created",
        "participant_org_ids": [str(uuid4())],
        "payload": payload,
        "stream_seq": seq,
    }


class _Result:
    def __init__(self, rows):
        self._rows = rows

    def mappings(self):
        return self._rows


class _ScriptedEngine:
    """Returns one scripted outbox page per query, in cursor order.

    A page may be an exception instance, which is raised instead, to model a
    database failure in the middle of a drain.
    """

    def __init__(self, pages):
        self.pages = list(pages)

    def connect(self):
        return self

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args):
        return None

    async def execute(self, _query):
        page = self.pages.pop(0) if self.pages else []
        if isinstance(page, Exception):
            raise page
        return _Result(page)


def _worker(pages, *, batch_size=500):
    bus = EventBus()
    queues = {channel: bus.subscribe(channel) for channel in ("prices", "orderbook")}
    return MarketEventDispatcher(_ScriptedEngine(pages), bus, batch_size=batch_size), queues


def _drain_queue(queue) -> list[dict]:
    messages = []
    while not queue.empty():
        messages.append(queue.get_nowait())
    return messages


@pytest.mark.parametrize("channel", ["prices", "orderbook"])
async def test_public_refresh_has_no_id_and_the_original_payload(channel):
    dispatcher, queues = _worker(
        [[_row(11, public=True), _row(12, public=False), _row(13, public=True)]]
    )

    await dispatcher._drain_new_events()

    (message,) = _drain_queue(queues[channel])
    assert message["event"] == PUBLIC_INVALIDATION_EVENT
    assert message["data"] == {"schema_version": 1, "resync_required": True}
    assert message["data"] == PUBLIC_INVALIDATION_PAYLOAD
    assert "seq" not in message
    frame = stream._format_market_event(
        message.get("seq"), message["event"], message["data"]
    )
    assert frame.startswith("event: market_invalidated\n")
    assert "id:" not in frame


async def test_private_only_rows_publish_no_public_refresh():
    dispatcher, queues = _worker([[_row(5, public=False), _row(6, public=False)]])

    await dispatcher._drain_new_events()

    assert all(queue.empty() for queue in queues.values())


async def test_each_drain_with_a_marker_publishes_one_refresh():
    dispatcher, queues = _worker(
        [
            [_row(3, public=True)],
            [],  # idle poll: nothing to signal
            [_row(9, public=True), _row(10, public=True)],
        ]
    )

    for _ in range(3):
        await dispatcher._drain_new_events()

    assert len(_drain_queue(queues["orderbook"])) == 2


async def test_failed_drain_step_does_not_lose_a_public_marker():
    # The first page (a full batch) holds a marker; the second page fails.
    # The cursor has already moved past the marker row, so the marker must
    # survive on the dispatcher and be published by the next drain.
    dispatcher, queues = _worker(
        [[_row(1, public=True)], RuntimeError("database went away")],
        batch_size=1,
    )

    with pytest.raises(RuntimeError):
        await dispatcher._drain_new_events()
    assert queues["orderbook"].empty()

    await dispatcher._drain_new_events()

    for channel in ("prices", "orderbook"):
        (message,) = _drain_queue(queues[channel])
        assert message["event"] == PUBLIC_INVALIDATION_EVENT
        assert "seq" not in message

    # Published once: a further drain with no new marker stays silent.
    await dispatcher._drain_new_events()
    assert all(queue.empty() for queue in queues.values())

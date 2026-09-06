"""Unit tests for the in-process event bus."""
import asyncio
import pytest

from app.services.event_bus import EventBus


@pytest.fixture
def bus():
    """Fresh EventBus for each test (not the singleton)."""
    return EventBus()


# ---------------------------------------------------------------------------
# subscribe / unsubscribe
# ---------------------------------------------------------------------------

def test_subscribe_returns_queue(bus: EventBus):
    queue = bus.subscribe("prices")
    assert isinstance(queue, asyncio.Queue)
    assert queue.maxsize == 100
    assert bus._channels["prices"] == {queue}


def test_channel_deleted_when_last_subscriber_leaves(bus: EventBus):
    q1 = bus.subscribe("prices")
    q2 = bus.subscribe("prices")

    bus.unsubscribe("prices", q1)
    assert bus._channels["prices"] == {q2}

    bus.unsubscribe("prices", q2)
    assert "prices" not in bus._channels

    bus.unsubscribe("prices", q2)  # repeated unsubscribe is a no-op
    assert "prices" not in bus._channels


# ---------------------------------------------------------------------------
# publish
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_publish_delivers_to_all_subscribers(bus: EventBus):
    q1 = bus.subscribe("orderbook")
    q2 = bus.subscribe("orderbook")

    await bus.publish("orderbook", "order_created", {"id": "abc"})

    for queue in (q1, q2):
        msg = queue.get_nowait()
        assert set(msg) == {"event", "data", "timestamp"}
        assert msg["event"] == "order_created"
        assert msg["data"] == {"id": "abc"}
        assert "T" in msg["timestamp"]


@pytest.mark.asyncio
async def test_publish_to_empty_channel_is_noop(bus: EventBus):
    """Publishing to a channel with no subscribers should not raise."""
    await bus.publish("nonexistent", "test_event", {"x": 1})


@pytest.mark.asyncio
async def test_publish_skips_full_queues(bus: EventBus):
    """When a queue is full, publish should drop the message and remove the queue."""
    queue = bus.subscribe("prices")

    # Fill the queue to capacity (maxsize=100)
    for i in range(100):
        await bus.publish("prices", "tick", {"i": i})

    # Queue is now full; next publish should evict it
    await bus.publish("prices", "tick", {"i": 100})

    # The full queue should have been discarded
    assert queue not in bus._channels.get("prices", set())


# ---------------------------------------------------------------------------
# broadcast
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_broadcast_hits_all_channels(bus: EventBus):
    q_prices = bus.subscribe("prices")
    q_trades = bus.subscribe("trades")
    q_orderbook = bus.subscribe("orderbook")

    await bus.broadcast("system_event", {"msg": "maintenance"})

    for q in (q_prices, q_trades, q_orderbook):
        msg = q.get_nowait()
        assert msg["event"] == "system_event"
        assert msg["data"] == {"msg": "maintenance"}


@pytest.mark.asyncio
async def test_broadcast_with_no_channels(bus: EventBus):
    """Broadcast with zero channels should be a no-op."""
    await bus.broadcast("ping", {})

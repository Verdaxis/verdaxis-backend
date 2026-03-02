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


def test_subscribe_registers_channel(bus: EventBus):
    bus.subscribe("prices")
    assert "prices" in bus._channels
    assert len(bus._channels["prices"]) == 1


def test_unsubscribe_removes_queue(bus: EventBus):
    queue = bus.subscribe("prices")
    bus.unsubscribe("prices", queue)
    # Channel should be cleaned up entirely when last subscriber leaves
    assert "prices" not in bus._channels


def test_unsubscribe_idempotent(bus: EventBus):
    """Unsubscribing a queue that was already removed should not raise."""
    queue = bus.subscribe("prices")
    bus.unsubscribe("prices", queue)
    bus.unsubscribe("prices", queue)  # second call should be a no-op


def test_unsubscribe_keeps_other_subscribers(bus: EventBus):
    q1 = bus.subscribe("prices")
    q2 = bus.subscribe("prices")
    bus.unsubscribe("prices", q1)
    assert "prices" in bus._channels
    assert q2 in bus._channels["prices"]
    assert q1 not in bus._channels["prices"]


# ---------------------------------------------------------------------------
# publish
# ---------------------------------------------------------------------------

@pytest.mark.asyncio
async def test_publish_delivers_to_all_subscribers(bus: EventBus):
    q1 = bus.subscribe("orderbook")
    q2 = bus.subscribe("orderbook")

    await bus.publish("orderbook", "order_created", {"id": "abc"})

    msg1 = q1.get_nowait()
    msg2 = q2.get_nowait()
    assert msg1["event"] == "order_created"
    assert msg1["data"] == {"id": "abc"}
    assert "timestamp" in msg1
    assert msg2["event"] == "order_created"


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


@pytest.mark.asyncio
async def test_publish_does_not_crash_on_full_queue(bus: EventBus):
    """Regression: ensure no exception propagates when a queue is full."""
    bus.subscribe("trades")
    q = list(bus._channels["trades"])[0]

    # Fill it
    for _ in range(100):
        q.put_nowait({"event": "x", "data": {}, "timestamp": ""})

    # This must not raise
    await bus.publish("trades", "trade_created", {"id": "123"})


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


# ---------------------------------------------------------------------------
# cleanup semantics
# ---------------------------------------------------------------------------

def test_channel_deleted_when_last_subscriber_leaves(bus: EventBus):
    q1 = bus.subscribe("prices")
    q2 = bus.subscribe("prices")

    bus.unsubscribe("prices", q1)
    assert "prices" in bus._channels  # still has q2

    bus.unsubscribe("prices", q2)
    assert "prices" not in bus._channels  # fully cleaned up


@pytest.mark.asyncio
async def test_publish_message_structure(bus: EventBus):
    """Verify the exact message structure published to queues."""
    queue = bus.subscribe("test_channel")
    await bus.publish("test_channel", "my_event", {"key": "value"})

    msg = queue.get_nowait()
    assert set(msg.keys()) == {"event", "data", "timestamp"}
    assert msg["event"] == "my_event"
    assert msg["data"] == {"key": "value"}
    # timestamp should be an ISO 8601 string
    assert "T" in msg["timestamp"]

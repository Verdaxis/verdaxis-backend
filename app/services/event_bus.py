"""In-process event bus for SSE broadcasting."""
import asyncio
from collections import defaultdict
from typing import Any

from datetime import datetime, UTC


MAX_SUBSCRIBERS_PER_CHANNEL = 200
SUBSCRIBER_QUEUE_SIZE = 100


class SubscriberQueue(asyncio.Queue):
    """Subscriber queue that can report a terminal transport failure."""

    def __init__(self) -> None:
        super().__init__(maxsize=SUBSCRIBER_QUEUE_SIZE)
        self.terminal_message: dict[str, Any] | None = None

    def terminate_for_overflow(self) -> None:
        """Replace an undeliverable backlog with one explicit reset."""
        if self.terminal_message is not None:
            return
        message = {
            "event": "reset",
            "data": {
                "schema_version": 1,
                "reason": "subscriber_overflow",
                "resync_required": True,
            },
            "timestamp": datetime.now(UTC).isoformat(),
            "terminal": True,
        }
        self.terminal_message = message
        while not self.empty():
            self.get_nowait()
        self.put_nowait(message)


class EventBus:
    """Singleton pub/sub for broadcasting events to SSE clients."""

    def __init__(self):
        self._channels: dict[str, set[SubscriberQueue]] = defaultdict(set)

    def subscribe(self, channel: str) -> SubscriberQueue | None:
        """Subscribe to a channel. Returns a Queue that receives events, or None if at capacity."""
        if len(self._channels[channel]) >= MAX_SUBSCRIBERS_PER_CHANNEL:
            return None
        queue = SubscriberQueue()
        self._channels[channel].add(queue)
        return queue

    def unsubscribe(self, channel: str, queue: asyncio.Queue):
        """Remove a subscriber."""
        subscribers = self._channels.get(channel)
        if subscribers is None:
            return
        subscribers.discard(queue)
        if not subscribers:
            del self._channels[channel]

    async def publish(self, channel: str, event_type: str, data: Any, *, seq: int | None = None):
        """Publish an event to all subscribers of a channel.

        ``seq`` carries the durable stream sequence for messages fanned out
        from the market event outbox (used as the SSE ``id:`` field); local
        best-effort events omit it.
        """
        message = {
            "event": event_type,
            "data": data,
            "timestamp": datetime.now(UTC).isoformat(),
        }
        if seq is not None:
            message["seq"] = seq
        dead_queues: list[SubscriberQueue] = []
        for queue in list(self._channels.get(channel, set())):
            try:
                queue.put_nowait(message)
            except asyncio.QueueFull:
                queue.terminate_for_overflow()
                dead_queues.append(queue)
        # Clean up dead/full queues
        for q in dead_queues:
            self._channels[channel].discard(q)

    async def broadcast(self, event_type: str, data: Any):
        """Broadcast to ALL channels (global events)."""
        for channel in list(self._channels.keys()):
            await self.publish(channel, event_type, data)


# Singleton instance
event_bus = EventBus()

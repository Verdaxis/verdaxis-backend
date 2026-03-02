"""In-process event bus for SSE broadcasting."""
import asyncio
from collections import defaultdict
from typing import Any

from datetime import datetime, UTC


class EventBus:
    """Singleton pub/sub for broadcasting events to SSE clients."""

    def __init__(self):
        self._channels: dict[str, set[asyncio.Queue]] = defaultdict(set)

    def subscribe(self, channel: str) -> asyncio.Queue:
        """Subscribe to a channel. Returns a Queue that receives events."""
        queue: asyncio.Queue = asyncio.Queue(maxsize=100)
        self._channels[channel].add(queue)
        return queue

    def unsubscribe(self, channel: str, queue: asyncio.Queue):
        """Remove a subscriber."""
        self._channels[channel].discard(queue)
        if not self._channels[channel]:
            del self._channels[channel]

    async def publish(self, channel: str, event_type: str, data: Any):
        """Publish an event to all subscribers of a channel."""
        message = {
            "event": event_type,
            "data": data,
            "timestamp": datetime.now(UTC).isoformat(),
        }
        dead_queues = []
        for queue in self._channels.get(channel, set()):
            try:
                queue.put_nowait(message)
            except asyncio.QueueFull:
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

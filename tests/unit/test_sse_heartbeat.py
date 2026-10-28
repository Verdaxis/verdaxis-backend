"""SSE heartbeat contract: readable system status on every market stream.

Contract (the frontend is built against exactly this):
    event: heartbeat
    data: {"status": "...", "message": <str|null>, "server_time": "<ISO-8601 UTC>"}

with no ``id:`` line, sent right after a stream opens and in place of the old
``: keepalive`` comment every 30 idle seconds.
"""

import asyncio
import json
from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch
from uuid import uuid4

import pytest
from pydantic import ValidationError

from app.config import Settings
from app.routers import stream
from app.services.event_bus import EventBus, SubscriberQueue

pytestmark = pytest.mark.asyncio


class _Request:
    """Reports a disconnect after ``allowed_reads`` is_disconnected checks."""

    def __init__(self, allowed_reads: int):
        self.headers = {}
        self._allowed_reads = allowed_reads
        self._reads = 0

    async def is_disconnected(self):
        self._reads += 1
        return self._reads > self._allowed_reads


class _NoSubscriptionBus:
    def subscribe(self, _channel):
        return None

    def unsubscribe(self, _channel, _queue):
        raise AssertionError("nothing was subscribed")


def _parse_heartbeat(frame: str) -> dict:
    lines = frame.split("\n")
    assert lines[0] == "event: heartbeat"
    assert lines[1].startswith("data: ")
    assert lines[2:] == ["", ""], "frame must end with one blank line"
    assert not any(line.startswith("id:") for line in lines)
    return json.loads(lines[1][len("data: "):])


def _settings(**overrides) -> Settings:
    return Settings(
        DATABASE_URL="sqlite+aiosqlite:///:memory:",
        JWT_SECRET="test-secret-key-that-is-at-least-32-characters-long",
        ENVIRONMENT="test",
        RELEASE_SHA="test",
        **overrides,
    )


def _is_heartbeat(frame: str) -> bool:
    return frame.startswith("event: heartbeat\n")


def _ids(frames: list[str]) -> list[int]:
    return [int(frame.split("\n", 1)[0][len("id: "):]) for frame in frames if frame.startswith("id:")]


# --- frame format -----------------------------------------------------------


async def test_heartbeat_frame_has_contract_fields_and_no_id():
    with patch.object(stream, "settings", SimpleNamespace(SYSTEM_STATUS="degraded", SYSTEM_STATUS_MESSAGE="Slow fills")):
        frame = stream._format_heartbeat_event()

    body = _parse_heartbeat(frame)
    assert list(body) == ["status", "message", "server_time"]
    assert body["status"] == "degraded"
    assert body["message"] == "Slow fills"
    assert body["server_time"].endswith("Z")
    parsed = datetime.fromisoformat(body["server_time"].replace("Z", "+00:00"))
    assert parsed.tzinfo is not None and parsed.utcoffset().total_seconds() == 0
    assert abs((datetime.now(timezone.utc) - parsed).total_seconds()) < 5


async def test_heartbeat_message_is_null_by_default():
    body = _parse_heartbeat(stream._format_heartbeat_event())
    assert body["status"] == "operational"
    assert body["message"] is None


async def test_heartbeat_message_cannot_break_sse_framing():
    hostile = "line1\n\nid: 99\nevent: error\ndata: x"
    with patch.object(stream, "settings", SimpleNamespace(SYSTEM_STATUS="maintenance", SYSTEM_STATUS_MESSAGE=hostile)):
        frame = stream._format_heartbeat_event()

    assert frame.count("\n\n") == 1 and frame.endswith("\n\n")
    assert _parse_heartbeat(frame)["message"] == hostile


# --- settings ---------------------------------------------------------------


async def test_settings_defaults_are_operational_without_message():
    configured = _settings()
    assert configured.SYSTEM_STATUS == "operational"
    assert configured.SYSTEM_STATUS_MESSAGE is None


@pytest.mark.parametrize("status", ["operational", "degraded", "maintenance"])
async def test_settings_accept_each_status(status):
    assert _settings(SYSTEM_STATUS=status).SYSTEM_STATUS == status


async def test_settings_normalize_status_case_and_whitespace():
    assert _settings(SYSTEM_STATUS=" Degraded ").SYSTEM_STATUS == "degraded"


@pytest.mark.parametrize("status", ["", "down", "ok", "outage"])
async def test_settings_reject_unknown_status(status):
    with pytest.raises(ValidationError):
        _settings(SYSTEM_STATUS=status)


async def test_settings_message_is_bounded_and_blank_means_none():
    assert _settings(SYSTEM_STATUS_MESSAGE="x" * 280).SYSTEM_STATUS_MESSAGE == "x" * 280
    assert _settings(SYSTEM_STATUS_MESSAGE="   ").SYSTEM_STATUS_MESSAGE is None
    assert _settings(SYSTEM_STATUS_MESSAGE="  Planned work  ").SYSTEM_STATUS_MESSAGE == "Planned work"
    with pytest.raises(ValidationError):
        _settings(SYSTEM_STATUS_MESSAGE="x" * 281)


async def test_settings_read_status_from_environment(monkeypatch):
    monkeypatch.setenv("SYSTEM_STATUS", "maintenance")
    monkeypatch.setenv("SYSTEM_STATUS_MESSAGE", "Back at 14:00 UTC")
    configured = _settings()
    assert (configured.SYSTEM_STATUS, configured.SYSTEM_STATUS_MESSAGE) == (
        "maintenance",
        "Back at 14:00 UTC",
    )


# --- public streams (prices, orderbook) -------------------------------------


@pytest.mark.parametrize("channel", ["prices", "orderbook"])
async def test_public_stream_sends_initial_heartbeat_before_any_event(channel):
    bus = EventBus()
    request = _Request(allowed_reads=1)
    with patch.object(stream, "event_bus", bus):
        generator = stream._sse_generator(request, channel)
        first = await generator.__anext__()
        await generator.aclose()

    assert _is_heartbeat(first)
    _parse_heartbeat(first)
    assert channel not in bus._channels  # closing the stream unsubscribed


async def test_public_stream_idle_heartbeat_replaces_keepalive():
    bus = EventBus()
    with (
        patch.object(stream, "event_bus", bus),
        patch.object(stream, "_HEARTBEAT_INTERVAL_SECONDS", 0.01),
    ):
        frames = [frame async for frame in stream._sse_generator(_Request(allowed_reads=3), "prices")]

    assert len(frames) == 4  # initial + one per idle interval
    assert all(_is_heartbeat(frame) for frame in frames)
    assert not any("keepalive" in frame for frame in frames)


async def test_public_stream_events_keep_their_frames_between_heartbeats():
    queue = SubscriberQueue()
    queue.put_nowait({"event": "price_update", "data": {"p": 1}, "seq": 7})
    fake_bus = SimpleNamespace(subscribe=lambda _channel: queue, unsubscribe=lambda *_args: None)
    with (
        patch.object(stream, "event_bus", fake_bus),
        patch.object(stream, "_HEARTBEAT_INTERVAL_SECONDS", 0.01),
    ):
        frames = [frame async for frame in stream._sse_generator(_Request(allowed_reads=2), "prices")]

    assert _is_heartbeat(frames[0])
    assert frames[1] == 'id: 7\nevent: price_update\ndata: {"p": 1}\n\n'
    assert _is_heartbeat(frames[2])


@pytest.mark.parametrize("channel", ["prices", "orderbook"])
async def test_public_stream_too_many_connections_frame_is_unchanged(channel):
    with patch.object(stream, "event_bus", _NoSubscriptionBus()):
        frames = [frame async for frame in stream._sse_generator(_Request(allowed_reads=5), channel)]

    assert frames == ['event: error\ndata: {"error": "Too many connections"}\n\n']


async def test_private_stream_too_many_connections_frame_is_unchanged():
    with patch.object(stream, "event_bus", _NoSubscriptionBus()):
        frames = [
            frame
            async for frame in stream._private_sse_generator(
                _Request(allowed_reads=5), "trades:x", uuid4(), uuid4(), {"exp": 10_000}
            )
        ]

    assert frames == ['event: error\ndata: {"error": "Too many connections"}\n\n']


# --- private stream (trades) ------------------------------------------------


def _event(seq: int):
    return SimpleNamespace(stream_seq=seq, event_type="proof_marker", payload={"marker": seq})


class _ReplaySession:
    async def __aenter__(self):
        return self

    async def __aexit__(self, *_exc_info):
        return None

    async def rollback(self):
        return None


def _private_stream(request, queue, *, last_event_id=None, exp=10_000, replay=(), validate=None):
    """Run the private message generator against stubbed storage/authorization."""
    patches = [
        patch.object(stream, "AsyncSessionLocal", side_effect=lambda: _ReplaySession()),
        patch.object(
            stream,
            "fetch_replay_high_water",
            new=AsyncMock(return_value=max((e.stream_seq for e in replay), default=0)),
        ),
        patch.object(
            stream,
            "fetch_events_for_org",
            new=AsyncMock(side_effect=[list(replay), []]),
        ),
        patch.object(
            stream,
            "_validate_private_stream_authorization",
            new=validate or AsyncMock(return_value=None),
        ),
        patch.object(stream.time, "time", return_value=0),
    ]
    return patches, stream._private_sse_messages(
        request, queue, uuid4(), uuid4(), {"exp": exp}, last_event_id
    )


async def _collect(patches, generator):
    for item in patches:
        item.start()
    try:
        return [frame async for frame in generator]
    finally:
        for item in patches:
            item.stop()


async def test_private_initial_heartbeat_follows_replay_and_precedes_live_events():
    queue = SubscriberQueue()
    for seq in (6, 4):  # 4 duplicates the replay; 6 is new live data
        queue.put_nowait({"event": "proof_marker", "data": {"marker": seq}, "seq": seq})

    patches, generator = _private_stream(
        _Request(allowed_reads=3), queue, last_event_id=0, replay=[_event(2), _event(4)]
    )
    # Short idle interval so the generator reaches the disconnect check.
    patches.append(patch.object(stream, "_HEARTBEAT_INTERVAL_SECONDS", 0.01))
    frames = await _collect(patches, generator)

    assert frames[0].startswith(": stream-origin")
    kinds = ["heartbeat" if _is_heartbeat(f) else f.split("\n", 1)[0] for f in frames[1:]]
    # Replay in order, then heartbeat, then only the not-yet-delivered live row.
    assert kinds[:4] == ["id: 2", "id: 4", "heartbeat", "id: 6"]
    assert set(kinds[4:]) <= {"heartbeat"}
    assert _ids(frames) == [2, 4, 6]
    _parse_heartbeat(frames[3])


async def test_private_heartbeat_does_not_change_cursor_or_replay_after_reconnect():
    queue = SubscriberQueue()
    patches, generator = _private_stream(_Request(allowed_reads=1), queue, last_event_id=None)
    patches.append(patch.object(stream, "_HEARTBEAT_INTERVAL_SECONDS", 0.01))
    frames = await _collect(patches, generator)

    assert [_is_heartbeat(f) for f in frames] == [False, True]
    assert _ids(frames) == []  # the heartbeat carries no cursor


async def test_private_expired_token_is_not_preceded_by_a_heartbeat():
    queue = SubscriberQueue()
    patches, generator = _private_stream(_Request(allowed_reads=5), queue, exp=-1)
    frames = await _collect(patches, generator)

    assert not any(_is_heartbeat(f) for f in frames)
    assert frames[-1].startswith("event: auth_expired\n")


async def test_private_revoked_authorization_is_not_preceded_by_a_heartbeat():
    queue = SubscriberQueue()
    patches, generator = _private_stream(
        _Request(allowed_reads=5),
        queue,
        validate=AsyncMock(side_effect=ValueError("revoked")),
    )
    frames = await _collect(patches, generator)

    assert not any(_is_heartbeat(f) for f in frames)
    assert frames[-1].startswith("event: auth_revoked\n")


async def test_private_idle_heartbeat_replaces_keepalive_on_a_30_second_cadence():
    """Wake-ups for the 15 s revalidation must not become 15 s heartbeats."""
    clock = {"now": 1000.0}

    async def fake_wait_for(awaitable, timeout):
        awaitable.close()
        clock["now"] += timeout
        raise asyncio.TimeoutError

    queue = SubscriberQueue()
    # time.time stays 0, so revalidation wakes recur every 15 simulated seconds.
    patches, generator = _private_stream(_Request(allowed_reads=6), queue)
    patches += [
        patch.object(stream.time, "monotonic", side_effect=lambda: clock["now"]),
        patch.object(stream.asyncio, "wait_for", fake_wait_for),
    ]
    frames = await _collect(patches, generator)

    assert not any("keepalive" in f for f in frames)
    # Loop iterations at t=0,15,30,45,60,75: heartbeats at 0, 30, 60 only.
    assert sum(_is_heartbeat(f) for f in frames) == 3
    assert clock["now"] - 1000.0 == pytest.approx(90.0)


async def test_private_live_event_resets_the_idle_heartbeat_deadline():
    clock = {"now": 1000.0}
    queue = SubscriberQueue()
    waits = {"count": 0}

    async def fake_wait_for(awaitable, timeout):
        awaitable.close()
        waits["count"] += 1
        clock["now"] += 25.0
        if waits["count"] == 1:
            return {"event": "proof_marker", "data": {}, "seq": 1}
        raise asyncio.TimeoutError

    patches, generator = _private_stream(_Request(allowed_reads=3), queue)
    patches += [
        patch.object(stream.time, "monotonic", side_effect=lambda: clock["now"]),
        patch.object(stream.asyncio, "wait_for", fake_wait_for),
    ]
    frames = await _collect(patches, generator)

    # t=1000 heartbeat (next due 1030); t=1025 live row restarts the window
    # (next due 1055); the t=1050 wake-up finds no heartbeat due. Without the
    # reset a heartbeat would be sent at t=1050.
    assert [_is_heartbeat(f) for f in frames[1:]] == [True, False]
    assert _ids(frames) == [1]

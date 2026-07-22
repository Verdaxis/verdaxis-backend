"""Four-worker proof for the durable shared SSE/outbox dispatcher (Stage 5).

Boots a real ``uvicorn --workers 4`` server against one disposable
PostgreSQL 17 database and proves, over real HTTP/SSE:

1. Cross-worker delivery — an event committed by the worker that handles an
   HTTP order placement reaches SSE subscribers held by OTHER workers
   (subscribers are spread across at least two distinct worker PIDs, so at
   least one receiving worker is not the producing worker).
2. Replay — a disconnected subscriber that reconnects with the standard
   ``Last-Event-ID`` header receives every event committed for its
   organization while it was away, exactly once and in sequence order.
3. Tenant isolation — under concurrent producers, a subscriber for
   organization X never observes organization Y's events and vice versa.

Workers are never reduced below four; the test asserts the master actually
spawned four worker processes.
"""
from __future__ import annotations

import asyncio
import contextlib
import os
import socket
import subprocess
import sys
import time
from pathlib import Path
from uuid import uuid4

import httpx
import pytest
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import create_access_token, create_stream_token
from app.services.market_events import commit_market_events, participant_market_event
from tests.postgres.test_market_routes import _seed_route_market

_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_WORKERS = 4
_EVENT_WAIT_SECONDS = 20.0

pytestmark = pytest.mark.asyncio


def _free_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _worker_pids(master_pid: int) -> set[int]:
    """PIDs of actual uvicorn worker children (excludes resource_tracker)."""
    children = set()
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            stat = (Path("/proc") / entry / "stat").read_text()
            cmdline = (Path("/proc") / entry / "cmdline").read_bytes()
        except OSError:
            continue
        # field 4 (after the parenthesised comm) is ppid
        ppid = int(stat.rsplit(")", 1)[1].split()[1])
        if ppid == master_pid and b"resource_tracker" not in cmdline:
            children.add(int(entry))
    return children


@pytest.fixture
async def sse_server(market_pg_url):
    """A real 4-worker Uvicorn server plus a seeded market on one database."""
    engine = create_async_engine(market_pg_url, pool_size=5, max_overflow=0)
    from sqlalchemy import text

    async with engine.begin() as connection:
        await connection.execute(
            text(
                "TRUNCATE TABLE market_event_outbox, trades, orderbook_orders, "
                "inventory_items, users, organizations, products, delivery_points, "
                "ports RESTART IDENTITY CASCADE"
            )
        )
    seeded = await _seed_route_market(engine)

    port = _free_loopback_port()
    process = subprocess.Popen(
        [
            sys.executable,
            "-m",
            "uvicorn",
            "app.main:app",
            "--workers",
            str(_WORKERS),
            "--host",
            "127.0.0.1",
            "--port",
            str(port),
            "--log-level",
            "warning",
        ],
        cwd=_BACKEND_ROOT,
        env={
            **os.environ,
            "DATABASE_URL": market_pg_url,
            "MIGRATOR_DATABASE_URL": market_pg_url,
            "ENVIRONMENT": "test",
        },
    )
    base_url = f"http://127.0.0.1:{port}"
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            deadline = time.monotonic() + 60
            while True:
                try:
                    response = await client.get(f"{base_url}/health/ready")
                    if response.status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                if process.poll() is not None:
                    raise RuntimeError("uvicorn exited before becoming ready")
                if time.monotonic() > deadline:
                    raise RuntimeError("uvicorn did not become ready in 60s")
                await asyncio.sleep(0.5)
        assert len(_worker_pids(process.pid)) == _WORKERS
        yield base_url, seeded, engine
    finally:
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)
        await engine.dispose()


class _Subscriber:
    """One live SSE connection with parsed frames and its origin worker PID."""

    def __init__(self, base_url: str, user_id, organization_id, *, last_event_id=None):
        self._base_url = base_url
        self._user_id = user_id
        self._organization_id = organization_id
        self._last_event_id = last_event_id
        self._stack = contextlib.AsyncExitStack()
        self._lines = None
        self.pid: int | None = None

    async def __aenter__(self) -> "_Subscriber":
        client = await self._stack.enter_async_context(
            httpx.AsyncClient(timeout=httpx.Timeout(10.0, read=_EVENT_WAIT_SECONDS))
        )
        headers = {}
        if self._last_event_id is not None:
            headers["Last-Event-ID"] = str(self._last_event_id)
        token = create_stream_token(self._user_id, self._organization_id)
        response = await self._stack.enter_async_context(
            client.stream(
                "GET",
                f"{self._base_url}/api/stream/trades",
                params={"stream_token": token},
                headers=headers,
            )
        )
        assert response.status_code == 200, await response.aread()
        self._lines = response.aiter_lines()
        origin = await asyncio.wait_for(self._read_frame(), timeout=10)
        assert origin["comment"].startswith("stream-origin pid="), origin
        self.pid = int(origin["comment"].split("=", 1)[1])
        return self

    async def __aexit__(self, *exc_info) -> None:
        await self._stack.aclose()

    async def _read_frame(self) -> dict:
        frame: dict = {}
        async for line in self._lines:
            if line == "":
                if frame:
                    return frame
                continue
            if line.startswith(":"):
                frame["comment"] = line[1:].strip()
                continue
            field, _, value = line.partition(":")
            frame[field] = value.strip()
        raise RuntimeError("SSE stream ended unexpectedly")

    async def next_event(self, timeout: float = _EVENT_WAIT_SECONDS) -> dict:
        """Next real event frame, skipping keepalives and comments."""
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise asyncio.TimeoutError("no SSE event before the deadline")
            frame = await asyncio.wait_for(self._read_frame(), timeout=remaining)
            if "event" in frame:
                return frame

    async def expect_no_event(self, quiet_seconds: float) -> None:
        try:
            frame = await self.next_event(timeout=quiet_seconds)
        except asyncio.TimeoutError:
            return
        raise AssertionError(f"unexpected event leaked to this tenant: {frame}")


async def _open_subscribers_on_two_workers(base_url, user_id, org_id, stack):
    """Open subscribers until they span >=2 distinct worker PIDs (max 24)."""
    subscribers: list[_Subscriber] = []
    for _ in range(24):
        subscriber = await stack.enter_async_context(
            _Subscriber(base_url, user_id, org_id)
        )
        subscribers.append(subscriber)
        if len({item.pid for item in subscribers}) >= 2 and len(subscribers) >= 3:
            return subscribers
    raise AssertionError(
        f"subscribers landed on one worker PID only: {[s.pid for s in subscribers]}"
    )


async def _commit_marker_events(factory, org_id, markers) -> None:
    async with factory() as session:
        for marker in markers:
            await commit_market_events(
                session,
                [
                    participant_market_event(
                        event_type="proof_marker",
                        aggregate_type="proof",
                        aggregate_id=uuid4(),
                        participant_org_ids=[org_id],
                        payload={"marker": marker},
                    )
                ],
            )


async def test_four_worker_durable_sse_dispatch(sse_server):
    base_url, seeded, engine = sse_server
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    buyer_org = seeded["buyer_org_id"]
    seller_org = seeded["seller_org_id"]

    # ---- Phase 1: an event committed by one worker's HTTP request reaches
    # subscribers held by other workers.
    async with contextlib.AsyncExitStack() as stack:
        subscribers = await _open_subscribers_on_two_workers(
            base_url, seeded["buyer_id"], buyer_org, stack
        )
        subscriber_pids = {item.pid for item in subscribers}
        assert len(subscriber_pids) >= 2

        async with httpx.AsyncClient(timeout=15.0) as client:
            response = await client.post(
                f"{base_url}/api/orderbook",
                json={
                    "side": "BID",
                    "product_id": str(seeded["product_id"]),
                    "delivery_point_id": str(seeded["point_id"]),
                    "quantity_mt": "50.00",
                    "price_per_mt_usd": "650.00",
                    "availability_window": "SPOT",
                },
                headers={
                    "Authorization": f"Bearer {create_access_token(str(seeded['buyer_id']))}",
                    "Idempotency-Key": "sse-proof-bid",
                },
            )
            assert response.status_code == 201, response.text
            order_id = response.json()["id"]

        first_seq = None
        for subscriber in subscribers:
            frame = await subscriber.next_event()
            assert frame["event"] == "order_created"
            assert f'"{order_id}"' in frame["data"]
            assert int(frame["id"]) > 0
            if first_seq is None:
                first_seq = int(frame["id"])
            # Every worker fans out the same durable sequence number.
            assert int(frame["id"]) == first_seq
        # The producing request was handled by exactly one worker; since the
        # receiving subscribers span >=2 PIDs, at least one delivery crossed
        # a process boundary.

    # ---- Phase 2: Last-Event-ID replay after disconnect.
    await _commit_marker_events(factory, buyer_org, ["missed-1", "missed-2"])
    async with _Subscriber(
        base_url, seeded["buyer_id"], buyer_org, last_event_id=first_seq
    ) as replayer:
        replay_one = await replayer.next_event()
        replay_two = await replayer.next_event()
        assert replay_one["event"] == replay_two["event"] == "proof_marker"
        replayed = replay_one["data"] + replay_two["data"]
        assert '"missed-1"' in replayed and '"missed-2"' in replayed
        assert '"missed-1"' not in replay_two["data"] or '"missed-1"' not in replay_one["data"]
        assert first_seq < int(replay_one["id"]) < int(replay_two["id"])

        # Live delivery resumes after replay on the same connection, without
        # duplicating replayed events.
        await _commit_marker_events(factory, buyer_org, ["live-after-replay"])
        live = await replayer.next_event()
        assert '"live-after-replay"' in live["data"]
        assert int(live["id"]) > int(replay_two["id"])

    # ---- Phase 3: tenant isolation under concurrent producers.
    x_markers = [f"x-{index}" for index in range(20)]
    y_markers = [f"y-{index}" for index in range(20)]
    async with (
        _Subscriber(base_url, seeded["buyer_id"], buyer_org) as subscriber_x,
        _Subscriber(base_url, seeded["seller_id"], seller_org) as subscriber_y,
    ):
        await asyncio.gather(
            _commit_marker_events(factory, buyer_org, x_markers),
            _commit_marker_events(factory, seller_org, y_markers),
        )

        async def _collect(subscriber, expected_count):
            frames = [await subscriber.next_event() for _ in range(expected_count)]
            sequences = [int(frame["id"]) for frame in frames]
            assert sequences == sorted(sequences)
            assert len(set(sequences)) == len(sequences)
            return [frame["data"] for frame in frames]

        data_x, data_y = await asyncio.gather(
            _collect(subscriber_x, len(x_markers)),
            _collect(subscriber_y, len(y_markers)),
        )
        assert {marker for marker in x_markers if any(marker in item for item in data_x)} == set(x_markers)
        assert {marker for marker in y_markers if any(marker in item for item in data_y)} == set(y_markers)
        assert not any("y-" in item for item in data_x)
        assert not any("x-" in item for item in data_y)

        # No further cross-tenant traffic arrives after the burst either.
        await asyncio.gather(
            subscriber_x.expect_no_event(3.0),
            subscriber_y.expect_no_event(3.0),
        )

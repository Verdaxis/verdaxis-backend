"""Durable shared SSE dispatch for the market event outbox.

Design (Stage 5 integration):

- Producers persist ``MarketEventOutbox`` rows inside their economic
  transaction (``app.services.market_events``) and emit a ``pg_notify`` that
  PostgreSQL delivers only on commit. NOTIFY is a lossy wake signal; the
  outbox table is the durable source of truth.
- One **sequencer leader** per database (a session-level PostgreSQL advisory
  lock elects it; any worker can take over when the holder dies) assigns a
  global monotonic ``stream_seq`` from the migration-owned
  ``market_event_stream_seq`` sequence to committed-but-unsequenced rows.
  Because assignment happens *after* the producing transaction commits and
  is serialized through the single leader, the visible maximum sequence only
  ever grows: a subscriber cursor of ``stream_seq > last_seen`` can never
  miss a row that becomes visible later. Sequence holes (crashed assignment
  transactions) are permitted and meaningless. The assignment UPDATE runs on
  the exact session holding the advisory lock, so leadership can never be
  lost while an assignment from the old leader is still in flight.
- Every worker runs a **hub**: it LISTENs for wakes (with a poll fallback
  for lost notifies), reads newly sequenced rows, and fans them out to the
  in-process event bus on org-bound channels (``trades:{organization_id}``).
  Channel identity always derives from the row's persisted participant list
  and, on the subscriber side, from the authenticated stream token — never
  from client input.
- Reconnecting SSE clients replay directly from the outbox with
  ``Last-Event-ID`` via :func:`fetch_events_for_org`.

Connection budget: each worker holds ONE dedicated (non-pool) database
connection for LISTEN + leadership; all queries run on the normal engine
pool in short transactions. See docs/market-event-dispatch.md for the
operational notes and the outbox prune policy (no automatic pruning ships).
"""
from __future__ import annotations

import asyncio
import logging
from typing import Any
from uuid import UUID

from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncEngine, AsyncSession

from app.models.market_event import MarketEventOutbox

logger = logging.getLogger(__name__)

# Single wake channel for both hops (producer -> sequencer -> hubs); readers
# only ever act on committed table state, so spurious wakes are harmless.
MARKET_EVENT_WAKE_CHANNEL = "verdaxis_market_events"

# Session-level advisory lock key electing the single sequencer leader.
# Arbitrary fixed 64-bit literal; must never be reused for another lock.
SEQUENCER_ADVISORY_LOCK_KEY = 72645_37260_22662_5

STREAM_CHANNEL_PREFIX = "trades:"

_POLL_SECONDS = 2.0
_BATCH_SIZE = 500

# asyncpg statement (positional $1): assignment always runs on the leader's
# dedicated listener connection, never on a pool connection (see
# _assign_pending_sequences for the fencing rationale).
_ASSIGN_SEQUENCES_SQL = """
    WITH pending AS (
        SELECT id
        FROM market_event_outbox
        WHERE stream_seq IS NULL
        ORDER BY created_at, id
        LIMIT $1
        FOR UPDATE SKIP LOCKED
    )
    UPDATE market_event_outbox AS outbox
    SET stream_seq = nextval('market_event_stream_seq'),
        dispatched_at = now(),
        delivery_attempts = outbox.delivery_attempts + 1
    FROM pending
    WHERE outbox.id = pending.id
    RETURNING outbox.stream_seq
"""


def stream_channel_for_org(organization_id: UUID | str) -> str:
    return f"{STREAM_CHANNEL_PREFIX}{organization_id}"


async def fetch_events_for_org(
    session: AsyncSession,
    organization_id: UUID | str,
    *,
    after_seq: int,
    limit: int = _BATCH_SIZE,
) -> list[MarketEventOutbox]:
    """Sequenced events for one organization, oldest first.

    Serves Last-Event-ID replay. Only rows with an assigned ``stream_seq``
    are visible on the stream, so replay and live delivery share one cursor
    space. PostgreSQL filters participants with a jsonb containment test;
    other dialects (the SQLite unit harness) filter in Python.
    """
    bind = session.get_bind()
    if bind is not None and bind.dialect.name == "postgresql":
        result = await session.execute(
            text(
                """
                SELECT id FROM market_event_outbox
                WHERE stream_seq > :after_seq
                  AND participant_org_ids::jsonb ? :org_id
                ORDER BY stream_seq
                LIMIT :limit
                """
            ),
            {
                "after_seq": after_seq,
                "org_id": str(organization_id),
                "limit": limit,
            },
        )
        ids = [row[0] for row in result]
        if not ids:
            return []
        rows = (
            (
                await session.execute(
                    MarketEventOutbox.__table__.select().where(
                        MarketEventOutbox.id.in_(ids)
                    )
                )
            )
            .mappings()
            .all()
        )
        by_id = {row["id"]: row for row in rows}
        return [_as_event(by_id[event_id]) for event_id in ids]

    result = await session.execute(
        MarketEventOutbox.__table__.select()
        .where(MarketEventOutbox.stream_seq > after_seq)
        .order_by(MarketEventOutbox.stream_seq)
    )
    org_key = str(organization_id)
    matched = [
        _as_event(row)
        for row in result.mappings()
        if org_key in [str(value) for value in row["participant_org_ids"]]
    ]
    return matched[:limit]


class _EventRow:
    """Minimal read-only projection shared by replay and hub fan-out."""

    __slots__ = ("id", "event_type", "payload", "participant_org_ids", "stream_seq")

    def __init__(self, mapping: Any) -> None:
        self.id = mapping["id"]
        self.event_type = mapping["event_type"]
        self.payload = mapping["payload"]
        self.participant_org_ids = mapping["participant_org_ids"]
        self.stream_seq = mapping["stream_seq"]


def _as_event(mapping: Any) -> _EventRow:
    return _EventRow(mapping)


class MarketEventDispatcher:
    """Per-worker dispatch runtime: leadership, sequencing, and hub fan-out."""

    def __init__(
        self,
        engine: AsyncEngine,
        bus: Any,
        *,
        poll_seconds: float = _POLL_SECONDS,
        batch_size: int = _BATCH_SIZE,
    ) -> None:
        self._engine = engine
        self._bus = bus
        self._poll_seconds = poll_seconds
        self._batch_size = batch_size
        self._sequencer_wake = asyncio.Event()
        self._hub_wake = asyncio.Event()
        self._listener_conn: Any = None
        self._tasks: list[asyncio.Task] = []
        self._is_leader = False
        self._cursor = 0
        self._stopping = False

    @property
    def enabled(self) -> bool:
        return self._engine.dialect.name == "postgresql"

    async def start(self) -> None:
        if not self.enabled:
            logger.info("market_event_dispatch_disabled", extra={"reason": "non-postgresql engine"})
            return
        self._cursor = await self._current_max_seq()
        await self._connect_listener()
        self._tasks = [
            asyncio.create_task(self._sequencer_loop(), name="market-event-sequencer"),
            asyncio.create_task(self._hub_loop(), name="market-event-hub"),
        ]

    async def stop(self) -> None:
        self._stopping = True
        for task in self._tasks:
            task.cancel()
        for task in self._tasks:
            try:
                await task
            except (asyncio.CancelledError, Exception):  # noqa: BLE001
                pass
        self._tasks = []
        if self._listener_conn is not None:
            try:
                await self._listener_conn.close()
            except Exception:  # noqa: BLE001
                pass
            self._listener_conn = None
        self._is_leader = False

    # -- wiring ------------------------------------------------------------

    async def _connect_listener(self) -> None:
        import asyncpg

        url = self._engine.url
        self._listener_conn = await asyncpg.connect(
            host=url.host,
            port=url.port or 5432,
            user=url.username,
            password=url.password,
            database=url.database,
        )
        await self._listener_conn.add_listener(
            MARKET_EVENT_WAKE_CHANNEL, self._on_notify
        )

    def _on_notify(self, *_args: Any) -> None:
        # Independent events per loop: one loop clearing its wake can never
        # swallow the other's; the poll timeout remains the backstop anyway.
        self._sequencer_wake.set()
        self._hub_wake.set()

    async def _wait_for_wake(self, wake: asyncio.Event) -> None:
        try:
            await asyncio.wait_for(wake.wait(), timeout=self._poll_seconds)
        except asyncio.TimeoutError:
            pass
        wake.clear()

    async def _ensure_listener(self) -> None:
        """Reconnect after a dropped LISTEN connection (e.g. PG restart).

        Losing this connection also releases the advisory lock, so leadership
        is re-contested from scratch on the fresh session.
        """
        if self._listener_conn is not None and not self._listener_conn.is_closed():
            return
        self._is_leader = False
        old = self._listener_conn
        self._listener_conn = None
        if old is not None:
            try:
                await old.close()
            except Exception:  # noqa: BLE001
                pass
        await self._connect_listener()
        logger.warning("market_event_listener_reconnected")

    async def _current_max_seq(self) -> int:
        async with self._engine.connect() as conn:
            value = (
                await conn.execute(
                    text("SELECT COALESCE(MAX(stream_seq), 0) FROM market_event_outbox")
                )
            ).scalar_one()
        return int(value)

    # -- sequencer (single leader) ------------------------------------------

    async def _try_acquire_leadership(self) -> bool:
        if self._is_leader:
            return True
        if self._listener_conn is None or self._listener_conn.is_closed():
            return False
        acquired = await self._listener_conn.fetchval(
            "SELECT pg_try_advisory_lock($1)", SEQUENCER_ADVISORY_LOCK_KEY
        )
        self._is_leader = bool(acquired)
        return self._is_leader

    async def _sequencer_loop(self) -> None:
        while not self._stopping:
            try:
                await self._ensure_listener()
                if not await self._try_acquire_leadership():
                    await self._wait_for_wake(self._sequencer_wake)
                    continue
                assigned = await self._assign_pending_sequences()
                if assigned:
                    # Wake every worker's hub (including our own) only after
                    # the assignment transaction committed.
                    await self._listener_conn.execute(
                        f"NOTIFY {MARKET_EVENT_WAKE_CHANNEL}"
                    )
                    continue  # drain quickly while there is a backlog
                await self._wait_for_wake(self._sequencer_wake)
            except asyncio.CancelledError:
                raise
            except Exception:  # noqa: BLE001
                logger.exception("market_event_sequencer_step_failed")
                await asyncio.sleep(self._poll_seconds)

    async def _assign_pending_sequences(self) -> int:
        # Leadership fence: the assignment UPDATE runs on the SAME
        # PostgreSQL session that holds the sequencer advisory lock. A
        # pool connection here would let a selectively-dropped listener
        # connection release the lock while this worker's assignment is
        # still in flight, so a new leader could assign concurrently and
        # commit sequences out of visibility order — permanently skipping
        # events past every hub cursor. On the lock-holder's own session,
        # losing the connection aborts the in-flight statement and
        # releases the lock atomically; no two assignments can overlap.
        rows = await self._listener_conn.fetch(
            _ASSIGN_SEQUENCES_SQL, self._batch_size
        )
        return len(rows)

    # -- hub (every worker) ---------------------------------------------------

    async def _hub_loop(self) -> None:
        while not self._stopping:
            try:
                await self._drain_new_events()
                await self._wait_for_wake(self._hub_wake)
            except asyncio.CancelledError:
                raise
            except Exception:  # noqa: BLE001
                logger.exception("market_event_hub_step_failed")
                await asyncio.sleep(self._poll_seconds)

    async def _drain_new_events(self) -> None:
        while True:
            async with self._engine.connect() as conn:
                result = await conn.execute(
                    MarketEventOutbox.__table__.select()
                    .where(MarketEventOutbox.stream_seq > self._cursor)
                    .order_by(MarketEventOutbox.stream_seq)
                    .limit(self._batch_size)
                )
                rows = [_as_event(row) for row in result.mappings()]
            for event in rows:
                await self._fan_out(event)
                self._cursor = event.stream_seq
            if len(rows) < self._batch_size:
                return

    async def _fan_out(self, event: _EventRow) -> None:
        for org_id in event.participant_org_ids:
            await self._bus.publish(
                stream_channel_for_org(org_id),
                event.event_type,
                event.payload,
                seq=event.stream_seq,
            )

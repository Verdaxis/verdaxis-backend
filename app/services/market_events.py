"""Durable, participant-scoped market event envelopes.

The source branch intentionally does not include a transport dispatcher.
Shared runtime integration may claim pending rows only after commit and mark
``dispatched_at`` after durable delivery.  Request handlers never call the
process-local event bus for private market lifecycle events.
"""
from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any
from uuid import UUID

from sqlalchemy.ext.asyncio import AsyncSession

from app.models.market_event import MarketEventOutbox


@dataclass(frozen=True)
class MarketEventEnvelope:
    event_type: str
    aggregate_type: str
    aggregate_id: str
    participant_org_ids: tuple[UUID, ...]
    payload: dict[str, Any]

    def __post_init__(self) -> None:
        participants = tuple(sorted(set(self.participant_org_ids), key=str))
        if not participants:
            raise ValueError("market events require at least one participant organization")
        if not self.event_type.strip() or not self.aggregate_type.strip():
            raise ValueError("market event type and aggregate type are required")
        if not str(self.aggregate_id).strip():
            raise ValueError("market event aggregate ID is required")
        object.__setattr__(self, "participant_org_ids", participants)

    @property
    def routing_key(self) -> str:
        """Compatibility projection; never a global market channel."""
        return "market-orgs:" + ",".join(str(value) for value in self.participant_org_ids)

    def __iter__(self):
        # Existing pure builder tests may unpack an envelope while transport
        # integration migrates from legacy channel tuples.
        yield self.routing_key
        yield self.event_type
        yield self.payload


CommittedMarketEvent = MarketEventEnvelope


def participant_market_event(
    *,
    event_type: str,
    aggregate_type: str,
    aggregate_id: UUID | str,
    participant_org_ids: Iterable[UUID],
    payload: dict[str, Any],
) -> MarketEventEnvelope:
    return MarketEventEnvelope(
        event_type=event_type,
        aggregate_type=aggregate_type,
        aggregate_id=str(aggregate_id),
        participant_org_ids=tuple(participant_org_ids),
        payload=dict(payload),
    )


async def enqueue_market_events(
    db: AsyncSession,
    events: Iterable[MarketEventEnvelope],
) -> tuple[UUID, ...]:
    """Persist envelopes before commit; no transport is invoked here."""
    rows: list[MarketEventOutbox] = []
    for event in events:
        if not isinstance(event, MarketEventEnvelope):
            raise TypeError("legacy channel tuples are not durable market events")
        row = MarketEventOutbox(
            event_type=event.event_type,
            aggregate_type=event.aggregate_type,
            aggregate_id=event.aggregate_id,
            participant_org_ids=[str(value) for value in event.participant_org_ids],
            payload=event.payload,
        )
        db.add(row)
        rows.append(row)
    if rows:
        await db.flush()
    return tuple(row.id for row in rows)


async def commit_market_events(
    db: AsyncSession,
    events: Iterable[MarketEventEnvelope],
) -> tuple[UUID, ...]:
    """Persist audit/outbox state and commit atomically; publish nothing."""
    row_ids = await enqueue_market_events(db, events)
    await db.commit()
    return row_ids

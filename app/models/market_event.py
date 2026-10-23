"""Durable market event envelopes awaiting shared transport integration."""
from __future__ import annotations

from datetime import UTC, datetime
import uuid

from sqlalchemy import (
    JSON,
    BigInteger,
    DateTime,
    Index,
    Integer,
    String,
    Text,
    UniqueConstraint,
    func,
    text,
)
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base
from app.market_constraints import postgresql_check


class MarketEventOutbox(Base):
    """Participant-scoped event persisted in the economic transaction."""

    __tablename__ = "market_event_outbox"
    __table_args__ = (
        Index("ix_market_event_outbox_pending", "dispatched_at", "created_at"),
        # Integration (Stage 5): the shared SSE dispatcher's sequencer claims
        # rows where stream_seq IS NULL in created_at order.
        Index(
            "ix_market_event_outbox_unsequenced",
            "created_at",
            postgresql_where=text("stream_seq IS NULL"),
        ),
        UniqueConstraint("stream_seq", name="uq_market_event_outbox_stream_seq"),
        postgresql_check(
            "json_array_length(participant_org_ids) > 0",
            name="ck_market_event_outbox_participants",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    event_type: Mapped[str] = mapped_column(String(80), nullable=False)
    aggregate_type: Mapped[str] = mapped_column(String(64), nullable=False)
    aggregate_id: Mapped[str] = mapped_column(String(100), nullable=False)
    participant_org_ids: Mapped[list[str]] = mapped_column(JSON, nullable=False)
    payload: Mapped[dict] = mapped_column(JSON, nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )
    dispatched_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    # Integration (Stage 5): global monotonic delivery sequence, assigned by
    # the single-leader sequencer after the producing transaction commits
    # (values come from the migration-owned market_event_stream_seq
    # sequence). NULL means not yet dispatched to the shared SSE transport;
    # Last-Event-ID replay is `stream_seq > cursor` scoped to the org.
    stream_seq: Mapped[int | None] = mapped_column(BigInteger, nullable=True)
    delivery_attempts: Mapped[int] = mapped_column(
        Integer, nullable=False, default=0, server_default="0"
    )
    last_error: Mapped[str | None] = mapped_column(Text, nullable=True)

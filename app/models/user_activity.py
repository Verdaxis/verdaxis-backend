"""Authenticated browsing and market-interest events."""

from __future__ import annotations

import uuid
from datetime import UTC, datetime

from sqlalchemy import (
    CheckConstraint,
    DateTime,
    ForeignKey,
    Index,
    Integer,
    PrimaryKeyConstraint,
    String,
    func,
)
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


class UserBrowsingEvent(Base):
    __tablename__ = "user_browsing_events"
    __table_args__ = (
        PrimaryKeyConstraint(
            "user_id",
            "event_id",
            name="pk_user_browsing_events",
        ),
        CheckConstraint(
            "action IN ('page_view', 'market_view', 'market_filter')",
            name="ck_user_browsing_events_action",
        ),
        CheckConstraint("consent_version = 2", name="ck_user_browsing_events_consent_version"),
        CheckConstraint(
            "length(page) BETWEEN 1 AND 64",
            name="ck_user_browsing_events_page_length",
        ),
        Index("ix_user_browsing_events_user_received", "user_id", "received_at"),
        Index("ix_user_browsing_events_received", "received_at"),
    )

    user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("users.id", ondelete="CASCADE"),
    )
    event_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True))
    # Deprecated compatibility marker. New intake stores NULL because privacy
    # terms are maintained outside this application.
    consent_version: Mapped[int | None] = mapped_column(Integer, nullable=True)
    action: Mapped[str] = mapped_column(String(32), nullable=False)
    page: Mapped[str] = mapped_column(String(64), nullable=False)
    market_product: Mapped[str | None] = mapped_column(String(32), nullable=True)
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), nullable=True)
    availability_window: Mapped[str | None] = mapped_column(String(16), nullable=True)
    received_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
    )

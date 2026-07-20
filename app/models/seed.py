"""Metadata for idempotent, explicitly classified seed runs."""
from datetime import UTC, datetime
import uuid

from sqlalchemy import DateTime, JSON, String, Text, UniqueConstraint, func
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


class SeedRun(Base):
    __tablename__ = "seed_runs"
    __table_args__ = (
        UniqueConstraint("seed_name", "environment", name="uq_seed_runs_name_environment"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    seed_name: Mapped[str] = mapped_column(String(80), nullable=False)
    environment: Mapped[str] = mapped_column(String(32), nullable=False)
    run_metadata: Mapped[dict | None] = mapped_column("metadata", JSON, nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now()
    )
    completed_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )


class MarketRowQuarantine(Base):
    """Auditable copy of one explicitly selected legacy market row."""

    __tablename__ = "market_row_quarantines"
    __table_args__ = (
        UniqueConstraint("source_table", "source_id", name="uq_market_row_quarantines_source"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    source_table: Mapped[str] = mapped_column(String(64), nullable=False)
    source_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), nullable=False)
    original_row: Mapped[dict] = mapped_column(JSON, nullable=False)
    dependencies: Mapped[dict] = mapped_column(JSON, nullable=False)
    environment: Mapped[str] = mapped_column(String(32), nullable=False)
    database_name: Mapped[str] = mapped_column(String(128), nullable=False)
    reason: Mapped[str] = mapped_column(Text, nullable=False)
    operator: Mapped[str] = mapped_column(String(255), nullable=False)
    reference: Mapped[str] = mapped_column(String(255), nullable=False)
    quarantined_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )

"""Immutable successful results for the bounded market command set."""
from datetime import UTC, datetime
import uuid

from sqlalchemy import CheckConstraint, DateTime, ForeignKey, Index, JSON, SmallInteger, String, Text, UniqueConstraint, func
from sqlalchemy.dialects.postgresql import JSONB, UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


_JSON_VARIANT = JSON().with_variant(JSONB(astext_type=Text()), "postgresql")


class MarketCommandResult(Base):
    """Append-only receipt written with the successful economic transaction."""

    __tablename__ = "market_command_results"
    __table_args__ = (
        UniqueConstraint("actor_user_id", "operation", "idempotency_key", name="uq_market_command_results_actor_operation_key"),
        Index("ix_market_command_results_resource_created", "resource_type", "resource_id", "created_at"),
        CheckConstraint(
            "operation IN ('trade.confirm', 'trade.decline', 'trade.deliver', "
            "'trade.pay', 'order.amend', 'order.cancel')",
            name="ck_market_command_results_operation",
        ),
        CheckConstraint("resource_type IN ('trade', 'order')", name="ck_market_command_results_resource_type"),
        CheckConstraint("length(btrim(idempotency_key)) BETWEEN 1 AND 255", name="ck_market_command_results_key"),
        CheckConstraint(
            "request_hash ~ '^[0-9a-f]{64}$'",
            name="ck_market_command_results_request_hash",
        ).ddl_if(dialect="postgresql"),
        CheckConstraint("response_status BETWEEN 200 AND 299", name="ck_market_command_results_response_status"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    actor_user_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("users.id", ondelete="RESTRICT"), nullable=False)
    effective_organization_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id", ondelete="RESTRICT"), nullable=False)
    support_context_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("market_support_contexts.id", ondelete="RESTRICT"), nullable=True)
    operation: Mapped[str] = mapped_column(String(32), nullable=False)
    idempotency_key: Mapped[str] = mapped_column(String(255), nullable=False)
    request_hash: Mapped[str] = mapped_column(String(64), nullable=False)
    resource_type: Mapped[str] = mapped_column(String(16), nullable=False)
    resource_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), nullable=False)
    response_status: Mapped[int] = mapped_column(SmallInteger, nullable=False)
    response_body: Mapped[dict | list | None] = mapped_column(_JSON_VARIANT, nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now(), nullable=False
    )

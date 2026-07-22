"""RFQ (Request for Quote) models — bilateral negotiation alongside the orderbook."""
import enum
import uuid
from datetime import datetime, UTC
from decimal import Decimal

from sqlalchemy import ForeignKey, Enum, String, Numeric, DateTime, Text, Boolean, Index, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.model_base import Base
from app.services.availability_windows import SPOT_WINDOW
from app.market_constraints import (
    RFQ_DOMAIN,
    RFQ_LIFECYCLE,
    RFQ_NUMERIC_VALUES,
    RFQ_QUOTE_DOMAIN,
    RFQ_QUOTE_NUMERIC_VALUES,
    postgresql_check,
)


class RFQStatus(str, enum.Enum):
    OPEN = "OPEN"
    QUOTED = "QUOTED"        # at least one quote received
    ACCEPTED = "ACCEPTED"    # buyer accepted a quote → trade created
    EXPIRED = "EXPIRED"
    CANCELLED = "CANCELLED"


class QuoteStatus(str, enum.Enum):
    PENDING = "PENDING"
    ACCEPTED = "ACCEPTED"
    DECLINED = "DECLINED"
    WITHDRAWN = "WITHDRAWN"


class RFQ(Base):
    __tablename__ = "rfqs"
    __table_args__ = (
        Index("ix_rfqs_buyer_org_id", "buyer_org_id"),
        Index("ix_rfqs_status", "status"),
        postgresql_check(RFQ_DOMAIN, name="ck_rfqs_domain"),
        postgresql_check(RFQ_NUMERIC_VALUES, name="ck_rfqs_numeric_values"),
        postgresql_check(RFQ_LIFECYCLE, name="ck_rfqs_lifecycle"),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    buyer_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    buyer_user_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("users.id"), nullable=True, index=True)
    product_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("products.id"), nullable=False
    )
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True
    )
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)
    target_price_per_mt: Mapped[Decimal | None] = mapped_column(Numeric(), nullable=True)
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False, default=SPOT_WINDOW)
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    is_anonymous: Mapped[bool] = mapped_column(Boolean, default=False)
    status: Mapped[RFQStatus] = mapped_column(
        Enum(RFQStatus, native_enum=False), default=RFQStatus.OPEN, nullable=False
    )
    accepted_quote_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("rfq_quotes.id"), nullable=True
    )
    trade_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("trades.id"), nullable=True
    )
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    # Relationships
    quotes: Mapped[list["RFQQuote"]] = relationship(
        back_populates="rfq",
        cascade="all, delete-orphan",
        foreign_keys="RFQQuote.rfq_id",
    )


class RFQQuote(Base):
    __tablename__ = "rfq_quotes"
    __table_args__ = (
        Index("ix_rfq_quotes_rfq_id", "rfq_id"),
        Index("ix_rfq_quotes_seller_org_id", "seller_org_id"),
        UniqueConstraint("rfq_id", "seller_org_id", name="uq_rfq_quotes_rfq_seller"),
        postgresql_check(RFQ_QUOTE_DOMAIN, name="ck_rfq_quotes_domain"),
        postgresql_check(RFQ_QUOTE_NUMERIC_VALUES, name="ck_rfq_quotes_numeric_values"),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    rfq_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("rfqs.id"), nullable=False
    )
    seller_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    seller_user_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("users.id"), nullable=True, index=True)
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    status: Mapped[QuoteStatus] = mapped_column(
        Enum(QuoteStatus, native_enum=False), default=QuoteStatus.PENDING, nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    rfq: Mapped["RFQ"] = relationship(back_populates="quotes", foreign_keys=[rfq_id])

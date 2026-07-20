"""RFQ (Request for Quote) models — bilateral negotiation alongside the orderbook."""
import enum
import uuid
from datetime import datetime, UTC
from decimal import Decimal

from sqlalchemy import ForeignKey, Enum, Index, String, Numeric, DateTime, Text, Boolean
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base
from app.services.availability_windows import SPOT_WINDOW


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
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    target_price_per_mt: Mapped[Decimal | None] = mapped_column(Numeric(10, 2), nullable=True)
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False, default=SPOT_WINDOW)
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    is_anonymous: Mapped[bool] = mapped_column(Boolean, default=False)
    status: Mapped[RFQStatus] = mapped_column(
        Enum(RFQStatus, native_enum=False, length=10), default=RFQStatus.OPEN, nullable=False
    )
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    # Relationships
    quotes: Mapped[list["RFQQuote"]] = relationship(back_populates="rfq", cascade="all, delete-orphan")


class RFQQuote(Base):
    __tablename__ = "rfq_quotes"
    __table_args__ = (Index("ix_rfq_quotes_rfq_id", "rfq_id"),)

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
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    status: Mapped[QuoteStatus] = mapped_column(
        Enum(QuoteStatus, native_enum=False, length=10), default=QuoteStatus.PENDING, nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    rfq: Mapped["RFQ"] = relationship(back_populates="quotes")

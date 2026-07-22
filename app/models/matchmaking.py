from sqlalchemy import String, ForeignKey, Enum, Numeric, DateTime, JSON, func, text
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime
from decimal import Decimal
from app.model_base import Base


class MatchStatus(str, enum.Enum):
    SUGGESTED = "SUGGESTED"
    VIEWED = "VIEWED"
    ACTED = "ACTED"
    DISMISSED = "DISMISSED"


class MatchSuggestion(Base):
    """
    A suggested match between a BID and an ASK order.
    Generated when a new order is placed that is compatible
    with existing orders on the opposite side.
    """
    __tablename__ = "match_suggestions"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    bid_order_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("orderbook_orders.id"), nullable=False)
    ask_order_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("orderbook_orders.id"), nullable=False)

    # Match quality score (0-100)
    score: Mapped[Decimal] = mapped_column(Numeric(5, 2), nullable=False)

    # JSON array of reasons: ["fuel_type_match", "region_match", "price_overlap", ...]
    match_reasons: Mapped[list] = mapped_column(JSON, default=list, server_default=text("'[]'"), nullable=False)

    status: Mapped[MatchStatus] = mapped_column(
        Enum(MatchStatus, native_enum=False, length=20),
        default=MatchStatus.SUGGESTED, nullable=False, server_default=text("'SUGGESTED'"),
    )

    # Who this suggestion is for (the org that placed the triggering order)
    recipient_org_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)

    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, server_default=func.now(), nullable=False)

    # Relationships
    bid_order = relationship("OrderBookOrder", foreign_keys=[bid_order_id])
    ask_order = relationship("OrderBookOrder", foreign_keys=[ask_order_id])
    recipient_org = relationship("Organization", foreign_keys=[recipient_org_id])

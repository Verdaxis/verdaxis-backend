from sqlalchemy import String, ForeignKey, Enum, Numeric, Date, DateTime, Boolean, JSON
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime, date
from decimal import Decimal
from app.database import Base


class FuelGrade(str, enum.Enum):
    CONVENTIONAL = "Conventional"
    GREEN = "Green"
    BIO = "Bio"


class AvailabilityWindow(str, enum.Enum):
    SPOT = "Spot"
    Q1_2025 = "Q1 2025"
    Q2_2025 = "Q2 2025"
    Q3_2025 = "Q3 2025"
    Q4_2025 = "Q4 2025"
    Q1_2026 = "Q1 2026"
    Q2_2026 = "Q2 2026"
    Q3_2026 = "Q3 2026"
    Q4_2026 = "Q4 2026"
    FORWARD_2027 = "Forward 2027"
    FORWARD_2028 = "Forward 2028"


class OrderSide(str, enum.Enum):
    BID = "BID"
    ASK = "ASK"


class OrderBookStatus(str, enum.Enum):
    OPEN = "OPEN"
    PARTIALLY_FILLED = "PARTIALLY_FILLED"
    FILLED = "FILLED"
    CANCELLED = "CANCELLED"
    EXPIRED = "EXPIRED"


class TradeStatus(str, enum.Enum):
    PENDING_CONFIRMATION = "PENDING_CONFIRMATION"
    CONFIRMED = "CONFIRMED"
    DELIVERED = "DELIVERED"
    PAID = "PAID"
    CANCELLED = "CANCELLED"
    DECLINED = "DECLINED"


class Initiator(str, enum.Enum):
    BUYER = "BUYER"
    SELLER = "SELLER"


class OrderBookOrder(Base):
    """
    Unified order book entry. Every entry is either a BID (buy) or ASK (sell).
    Replaces both PublicListing (ASK) and DirectOrder (BID).
    """
    __tablename__ = "orderbook_orders"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    organization_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    side: Mapped[OrderSide] = mapped_column(Enum(OrderSide, native_enum=False), nullable=False)

    # Product
    fuel_type: Mapped[str] = mapped_column(String(50), nullable=False)
    fuel_grade: Mapped[FuelGrade] = mapped_column(Enum(FuelGrade, native_enum=False), default=FuelGrade.CONVENTIONAL)
    region: Mapped[str] = mapped_column(String(50), nullable=False)

    # Optional location references
    port_id: Mapped[str | None] = mapped_column(String(50), nullable=True)
    vessel_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("vessels.id"), nullable=True)

    # Quantity
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    remaining_quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)

    # Price
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)

    # Timing
    availability_window: Mapped[AvailabilityWindow] = mapped_column(
        Enum(AvailabilityWindow, native_enum=False),
        default=AvailabilityWindow.SPOT
    )
    delivery_window_start: Mapped[date | None] = mapped_column(Date, nullable=True)
    delivery_window_end: Mapped[date | None] = mapped_column(Date, nullable=True)

    # ASK-specific
    certifications: Mapped[list | None] = mapped_column(JSON, default=list)
    is_verdaxis_verified: Mapped[bool] = mapped_column(Boolean, default=False)

    # Status
    status: Mapped[OrderBookStatus] = mapped_column(
        Enum(OrderBookStatus, native_enum=False),
        default=OrderBookStatus.OPEN
    )
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)

    # Timestamps
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow)

    # Relationships
    organization = relationship("Organization", back_populates="orderbook_orders")
    vessel = relationship("Vessel", foreign_keys=[vessel_id])
    bid_trades = relationship("Trade", foreign_keys="Trade.bid_order_id", back_populates="bid_order")
    ask_trades = relationship("Trade", foreign_keys="Trade.ask_order_id", back_populates="ask_order")

    @property
    def tier_label(self):
        """Derive tier from the owning organization (for ASK orders)."""
        if self.organization and self.organization.supplier_tier:
            return self.organization.supplier_tier
        from app.models.user import TierLabel
        return TierLabel.INDEPENDENT


class Trade(Base):
    """
    Matched transaction created when one side 'hits' the other's order.
    Replaces both the legacy Order (listing→buyer) and accepted DirectOrderOffer.
    """
    __tablename__ = "trades"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)

    # Link to orders (nullable for legacy trades migrated without matching order)
    bid_order_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("orderbook_orders.id"), nullable=True)
    ask_order_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("orderbook_orders.id"), nullable=True)

    # Parties
    buyer_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    seller_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    initiated_by: Mapped[Initiator] = mapped_column(Enum(Initiator, native_enum=False), nullable=False)

    # Trade details
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)

    # Status
    status: Mapped[TradeStatus] = mapped_column(
        Enum(TradeStatus, native_enum=False),
        default=TradeStatus.PENDING_CONFIRMATION
    )

    # Final deal details (populated on delivery)
    final_quantity_mt: Mapped[Decimal | None] = mapped_column(Numeric(12, 2), nullable=True)
    final_price_per_mt: Mapped[Decimal | None] = mapped_column(Numeric(10, 2), nullable=True)
    final_total_usd: Mapped[Decimal | None] = mapped_column(Numeric(14, 2), nullable=True)

    # Commission
    commission_rate_pct: Mapped[Decimal] = mapped_column(Numeric(5, 3), default=Decimal("0.5"))
    commission_amount_usd: Mapped[Decimal | None] = mapped_column(Numeric(12, 2), nullable=True)

    # Lifecycle timestamps
    confirmed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    delivered_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    paid_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    # Relationships
    bid_order = relationship("OrderBookOrder", foreign_keys=[bid_order_id], back_populates="bid_trades")
    ask_order = relationship("OrderBookOrder", foreign_keys=[ask_order_id], back_populates="ask_trades")
    buyer = relationship("Organization", foreign_keys=[buyer_id])
    seller = relationship("Organization", foreign_keys=[seller_id])
    commission = relationship("Commission", back_populates="trade", uselist=False)

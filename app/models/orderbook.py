from sqlalchemy import (
    Boolean,
    Date,
    DateTime,
    Enum,
    ForeignKey,
    Index,
    JSON,
    Numeric,
    SmallInteger,
    String,
    Text,
    UniqueConstraint,
    func,
    text,
)
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import UTC, date, datetime
from decimal import Decimal
from app.model_base import Base
from app.services.availability_windows import SPOT_WINDOW
from app.models.user import OrganizationProvenance
from app.market_constraints import (
    ORDER_DOMAIN,
    ORDER_LIFECYCLE,
    ORDER_NUMERIC_VALUES,
    TRADE_DOMAIN,
    TRADE_LIFECYCLE,
    TRADE_NUMERIC_VALUES,
    TRADE_SNAPSHOT,
    postgresql_check,
)


class FuelGrade(str, enum.Enum):
    CONVENTIONAL = "Conventional"
    GREEN = "Green"
    BIO = "Bio"
    E = "E"
    SYNTHETIC = "Synthetic"


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
    __table_args__ = (
        Index(
            "ix_orderbook_orders_active_slice_lookup",
            "side",
            "status",
            "product_id",
            "delivery_point_id",
            "availability_window",
            "created_at",
        ),
        Index(
            "ix_orderbook_orders_active_slice_expiry",
            "product_id",
            "delivery_point_id",
            "availability_window",
            "status",
            "expires_at",
        ),
        Index(
            "ix_orderbook_orders_public_aggregate",
            "status",
            "provenance",
            "product_id",
            "delivery_point_id",
            "availability_window",
            "side",
            "expires_at",
        ),
        Index(
            "ix_orderbook_orders_inventory_active",
            "inventory_item_id",
            "status",
            "expires_at",
        ),
        UniqueConstraint(
            "organization_id", "idempotency_operation", "idempotency_key",
            name="uq_orderbook_orders_org_operation_idempotency",
        ),
        postgresql_check(ORDER_DOMAIN, name="ck_orderbook_orders_domain"),
        postgresql_check(ORDER_NUMERIC_VALUES, name="ck_orderbook_orders_numeric_values"),
        postgresql_check(ORDER_LIFECYCLE, name="ck_orderbook_orders_lifecycle"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    organization_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    owner_user_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("users.id"), nullable=True, index=True)
    # Set only for listings published from managed inventory. Legacy and
    # manually-created orders remain NULL; cancellation must never guess.
    inventory_item_id: Mapped[uuid.UUID | None] = mapped_column(
        ForeignKey("inventory_items.id", ondelete="RESTRICT"), nullable=True, index=True
    )
    provenance: Mapped[OrganizationProvenance] = mapped_column(
        Enum(OrganizationProvenance, native_enum=False),
        nullable=False,
        default=OrganizationProvenance.UNKNOWN,
        server_default=OrganizationProvenance.UNKNOWN.value,
    )
    side: Mapped[OrderSide] = mapped_column(Enum(OrderSide, native_enum=False), nullable=False)

    # Product & Delivery Point (FK references)
    product_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("products.id"), nullable=False
    )
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True
    )

    # Optional location references
    port_id: Mapped[str | None] = mapped_column(String(50), nullable=True)
    vessel_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("vessels.id"), nullable=True)
    # inventory_item_id is declared above with the sec_fresh-shipped
    # ON DELETE SET NULL semantics (integration owns this column's DDL).

    # Quantity
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)
    remaining_quantity_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)

    # Price
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)

    # Timing
    availability_window: Mapped[str] = mapped_column(
        String(16),
        nullable=False,
        default=SPOT_WINDOW,
        server_default=text("'SPOT'"),
    )
    # Retained for historical import compatibility; canonical matching uses
    # availability_window and never derives a slice from these legacy dates.
    delivery_window_start: Mapped[date | None] = mapped_column(Date, nullable=True)
    delivery_window_end: Mapped[date | None] = mapped_column(Date, nullable=True)

    # ASK-specific
    certifications: Mapped[list | None] = mapped_column(
        JSON, default=list, server_default=text("'[]'")
    )
    certification_declared: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    certification_scheme: Mapped[str | None] = mapped_column(String(120), nullable=True)
    specification_standard: Mapped[str | None] = mapped_column(String(120), nullable=True)
    msds_available: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    is_verdaxis_verified: Mapped[bool] = mapped_column(
        Boolean, default=False, server_default=text("false"), nullable=False
    )

    # CI data (optional -- populated by supplier for ASK orders)
    carbon_intensity_gco2_mj: Mapped[Decimal | None] = mapped_column(
        Numeric(), nullable=True, comment="gCO2eq/MJ well-to-wake"
    )
    carbon_intensity_method: Mapped[str | None] = mapped_column(String(120), nullable=True)
    energy_density_mj_kg: Mapped[Decimal | None] = mapped_column(
        Numeric(), nullable=True, comment="MJ/kg lower heating value"
    )
    feedstock: Mapped[str | None] = mapped_column(String(255), nullable=True)
    origin: Mapped[str | None] = mapped_column(String(255), nullable=True)
    off_spec: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    off_spec_notes: Mapped[str | None] = mapped_column(Text, nullable=True)

    # Status
    status: Mapped[OrderBookStatus] = mapped_column(
        Enum(OrderBookStatus, native_enum=False),
        default=OrderBookStatus.OPEN,
        nullable=False,
        server_default=text("'OPEN'"),
    )
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    idempotency_key: Mapped[str | None] = mapped_column(String(255), nullable=True)
    idempotency_operation: Mapped[str | None] = mapped_column(String(64), nullable=True)
    idempotency_request_hash: Mapped[str | None] = mapped_column(String(64), nullable=True)

    # Timestamps
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        onupdate=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )

    # Relationships
    organization = relationship("Organization", back_populates="orderbook_orders")
    product = relationship("Product", lazy="selectin")
    delivery_point = relationship("DeliveryPoint", lazy="selectin")
    vessel = relationship("Vessel", foreign_keys=[vessel_id])
    inventory_item = relationship("InventoryItem")
    bid_trades = relationship("Trade", foreign_keys="Trade.bid_order_id", back_populates="bid_order")
    ask_trades = relationship("Trade", foreign_keys="Trade.ask_order_id", back_populates="ask_order")

    @property
    def tier_label(self):
        """Derive tier from the owning organization (for ASK orders)."""
        if self.organization and self.organization.supplier_tier:
            return self.organization.supplier_tier
        from app.models.user import TierLabel
        return TierLabel.INDEPENDENT

    # ---- Denormalized accessors for backward compatibility ----

    @property
    def fuel_type(self) -> str:
        """Derived from product relationship."""
        return self.product.fuel_type if self.product else ""

    @property
    def fuel_grade(self) -> str:
        """Derived from product relationship."""
        return self.product.fuel_grade if self.product else "Conventional"

    @property
    def market_product(self) -> str | None:
        """Derived from product relationship."""
        return self.product.market_product if self.product else None

    @property
    def region(self) -> str:
        """Derived from delivery_point relationship."""
        return self.delivery_point.region if self.delivery_point else ""

    @property
    def product_name(self) -> str:
        """Derived from product relationship."""
        return self.product.name if self.product else ""

    @property
    def delivery_point_name(self) -> str | None:
        """Derived from delivery_point relationship."""
        return self.delivery_point.name if self.delivery_point else None


class Trade(Base):
    """
    Matched transaction created when one side 'hits' the other's order.
    Replaces both the legacy Order (listing->buyer) and accepted DirectOrderOffer.
    """
    __tablename__ = "trades"
    __table_args__ = (
        Index("ix_trades_buyer", "buyer_id"),
        Index("ix_trades_seller", "seller_id"),
        Index("ix_trades_status", "status"),
        Index("ix_trades_status_confirmed_at", "status", "confirmed_at"),
        Index("ix_trades_buyer_created_at", "buyer_id", "created_at"),
        Index("ix_trades_seller_created_at", "seller_id", "created_at"),
        Index("ix_trades_snapshot_aggregation", "product_id", "market_product", "delivery_point_id", "availability_window", "confirmed_at"),
        UniqueConstraint(
            "initiator_org_id", "idempotency_operation", "idempotency_key",
            name="uq_trades_initiator_operation_idempotency",
        ),
        postgresql_check(TRADE_DOMAIN, name="ck_trades_domain"),
        postgresql_check(TRADE_NUMERIC_VALUES, name="ck_trades_numeric_values"),
        postgresql_check(TRADE_LIFECYCLE, name="ck_trades_lifecycle"),
        postgresql_check(TRADE_SNAPSHOT, name="ck_trades_snapshot"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)

    # Link to orders (nullable for legacy trades migrated without matching order)
    bid_order_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("orderbook_orders.id"), nullable=True)
    ask_order_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("orderbook_orders.id"), nullable=True)

    # Parties
    buyer_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    seller_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    buyer_user_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("users.id"), nullable=True, index=True)
    seller_user_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("users.id"), nullable=True, index=True)
    initiator_org_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    buyer_provenance: Mapped[OrganizationProvenance] = mapped_column(
        Enum(OrganizationProvenance, native_enum=False),
        nullable=False,
        default=OrganizationProvenance.UNKNOWN,
        server_default=OrganizationProvenance.UNKNOWN.value,
    )
    seller_provenance: Mapped[OrganizationProvenance] = mapped_column(
        Enum(OrganizationProvenance, native_enum=False),
        nullable=False,
        default=OrganizationProvenance.UNKNOWN,
        server_default=OrganizationProvenance.UNKNOWN.value,
    )
    initiated_by: Mapped[Initiator] = mapped_column(Enum(Initiator, native_enum=False), nullable=False)
    is_anonymous: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False, server_default="false")

    # Trade details
    product_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), ForeignKey("products.id"), nullable=True)
    product_name: Mapped[str | None] = mapped_column(String(120), nullable=True)
    fuel_type: Mapped[str | None] = mapped_column(String(64), nullable=True)
    fuel_grade: Mapped[str | None] = mapped_column(String(64), nullable=True)
    market_product: Mapped[str | None] = mapped_column(String(64), nullable=True)
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)
    delivery_point_name: Mapped[str | None] = mapped_column(String(120), nullable=True)
    delivery_point_region: Mapped[str | None] = mapped_column(String(120), nullable=True)
    availability_window: Mapped[str | None] = mapped_column(String(16), nullable=True)
    market_snapshot_version: Mapped[int | None] = mapped_column(
        SmallInteger,
        nullable=True,
        default=1,
        server_default="1",
    )
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)

    # Status
    status: Mapped[TradeStatus] = mapped_column(
        Enum(TradeStatus, native_enum=False),
        default=TradeStatus.PENDING_CONFIRMATION,
        nullable=False,
        server_default=text("'PENDING_CONFIRMATION'"),
    )

    # Final deal details (populated on delivery)
    final_quantity_mt: Mapped[Decimal | None] = mapped_column(Numeric(), nullable=True)
    final_price_per_mt: Mapped[Decimal | None] = mapped_column(Numeric(), nullable=True)
    final_total_usd: Mapped[Decimal | None] = mapped_column(Numeric(), nullable=True)

    # Commission
    commission_rate_pct: Mapped[Decimal] = mapped_column(
        Numeric(), nullable=False, default=Decimal("0.5"), server_default="0.5"
    )
    commission_amount_usd: Mapped[Decimal | None] = mapped_column(Numeric(), nullable=True)

    # Lifecycle timestamps
    confirmed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    delivered_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    paid_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )
    idempotency_key: Mapped[str | None] = mapped_column(String(255), nullable=True)
    idempotency_operation: Mapped[str | None] = mapped_column(String(64), nullable=True)
    idempotency_request_hash: Mapped[str | None] = mapped_column(String(64), nullable=True)

    # Relationships
    bid_order = relationship("OrderBookOrder", foreign_keys=[bid_order_id], back_populates="bid_trades")
    ask_order = relationship("OrderBookOrder", foreign_keys=[ask_order_id], back_populates="ask_trades")
    buyer = relationship("Organization", foreign_keys=[buyer_id])
    seller = relationship("Organization", foreign_keys=[seller_id])
    commission = relationship("Commission", back_populates="trade", uselist=False)

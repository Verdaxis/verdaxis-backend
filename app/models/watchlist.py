"""Watchlist models for market-radar slices, pins, and event history."""
import enum
import uuid
from datetime import datetime, UTC

from sqlalchemy import (
    Boolean,
    CheckConstraint,
    DateTime,
    Enum,
    ForeignKey,
    Index,
    JSON,
    String,
    text,
)
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base


class WatchlistKind(str, enum.Enum):
    CUSTOM = "CUSTOM"
    RADAR_DEFAULT = "RADAR_DEFAULT"
    LEGACY_ARCHIVED = "LEGACY_ARCHIVED"


class WatchlistTargetType(str, enum.Enum):
    SLICE = "SLICE"
    PIN = "PIN"


class WatchlistEventType(str, enum.Enum):
    SLICE_NEW_ORDER = "SLICE_NEW_ORDER"
    SLICE_BEST_PRICE_MOVED = "SLICE_BEST_PRICE_MOVED"
    SLICE_BENCHMARK_MOVED = "SLICE_BENCHMARK_MOVED"
    SLICE_WENT_QUIET = "SLICE_WENT_QUIET"
    PIN_PRICE_CHANGED = "PIN_PRICE_CHANGED"
    PIN_QUANTITY_CHANGED = "PIN_QUANTITY_CHANGED"
    PIN_PARTIALLY_FILLED = "PIN_PARTIALLY_FILLED"
    PIN_FILLED = "PIN_FILLED"
    PIN_WITHDRAWN = "PIN_WITHDRAWN"
    PIN_EXPIRED = "PIN_EXPIRED"


class Watchlist(Base):
    __tablename__ = "watchlists"
    __table_args__ = (
        Index(
            "uq_watchlists_user_default_radar",
            "user_id",
            unique=True,
            sqlite_where=text("kind = 'RADAR_DEFAULT'"),
            postgresql_where=text("kind = 'RADAR_DEFAULT'"),
        ),
        Index("ix_watchlists_user_kind", "user_id", "kind"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    user_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("users.id"), nullable=False)
    name: Mapped[str] = mapped_column(String(100), nullable=False)
    kind: Mapped[WatchlistKind] = mapped_column(
        Enum(WatchlistKind, native_enum=False),
        nullable=False,
        default=WatchlistKind.CUSTOM,
        server_default=WatchlistKind.CUSTOM.value,
    )
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    entries: Mapped[list["WatchlistEntry"]] = relationship(back_populates="watchlist", cascade="all, delete-orphan")
    targets: Mapped[list["WatchlistTarget"]] = relationship(back_populates="watchlist", cascade="all, delete-orphan")
    events: Mapped[list["WatchlistEvent"]] = relationship(back_populates="watchlist", cascade="all, delete-orphan")


class WatchlistEntry(Base):
    __tablename__ = "watchlist_entries"
    __table_args__ = (
        Index(
            "uq_watchlist_entries_watchlist_product_delivery_point",
            "watchlist_id",
            "product_id",
            "delivery_point_id",
            unique=True,
            sqlite_where=text("delivery_point_id IS NOT NULL"),
            postgresql_where=text("delivery_point_id IS NOT NULL"),
        ),
        Index(
            "uq_watchlist_entries_watchlist_product_no_delivery_point",
            "watchlist_id",
            "product_id",
            unique=True,
            sqlite_where=text("delivery_point_id IS NULL"),
            postgresql_where=text("delivery_point_id IS NULL"),
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    watchlist_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("watchlists.id", ondelete="CASCADE"), nullable=False)
    product_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("products.id"), nullable=False)
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    watchlist: Mapped["Watchlist"] = relationship(back_populates="entries")


class WatchlistTarget(Base):
    __tablename__ = "watchlist_targets"
    __table_args__ = (
        Index(
            "uq_watchlist_targets_slice_key",
            "watchlist_id",
            "market_product_code",
            "delivery_point_id",
            "availability_window_code",
            unique=True,
            sqlite_where=text("target_type = 'SLICE'"),
            postgresql_where=text("target_type = 'SLICE'"),
        ),
        Index(
            "uq_watchlist_targets_pin_order",
            "watchlist_id",
            "order_id",
            unique=True,
            sqlite_where=text("target_type = 'PIN'"),
            postgresql_where=text("target_type = 'PIN'"),
        ),
        Index("ix_watchlist_targets_watchlist_id", "watchlist_id"),
        Index("ix_watchlist_targets_order_id", "order_id"),
        CheckConstraint(
            "(target_type = 'SLICE' AND order_id IS NULL AND market_product_code IS NOT NULL AND delivery_point_id IS NOT NULL AND availability_window_code IS NOT NULL) "
            "OR (target_type = 'PIN' AND order_id IS NOT NULL AND market_product_code IS NOT NULL AND delivery_point_id IS NOT NULL AND availability_window_code IS NOT NULL)",
            name="ck_watchlist_targets_valid_shape",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    watchlist_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("watchlists.id", ondelete="CASCADE"), nullable=False)
    target_type: Mapped[WatchlistTargetType] = mapped_column(Enum(WatchlistTargetType, native_enum=False), nullable=False)
    market_product_code: Mapped[str | None] = mapped_column(String(64), nullable=True)
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)
    availability_window_code: Mapped[str | None] = mapped_column(String(32), nullable=True)
    order_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), ForeignKey("orderbook_orders.id"), nullable=True)
    snapshot_price_per_mt_usd: Mapped[float | None] = mapped_column(nullable=True)
    snapshot_quantity_mt: Mapped[float | None] = mapped_column(nullable=True)
    snapshot_remaining_quantity_mt: Mapped[float | None] = mapped_column(nullable=True)
    snapshot_status: Mapped[str | None] = mapped_column(String(32), nullable=True)
    snapshot_side: Mapped[str | None] = mapped_column(String(8), nullable=True)
    snapshot_market_product: Mapped[str | None] = mapped_column(String(64), nullable=True)
    snapshot_delivery_point_name: Mapped[str | None] = mapped_column(String(120), nullable=True)
    snapshot_availability_window: Mapped[str | None] = mapped_column(String(32), nullable=True)
    snapshot_counterparty_label: Mapped[str | None] = mapped_column(String(120), nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    watchlist: Mapped["Watchlist"] = relationship(back_populates="targets")
    delivery_point = relationship("DeliveryPoint", lazy="selectin")
    order = relationship("OrderBookOrder", lazy="selectin")
    events: Mapped[list["WatchlistEvent"]] = relationship(back_populates="target", cascade="all, delete-orphan")


class WatchlistEvent(Base):
    __tablename__ = "watchlist_events"
    __table_args__ = (
        Index("ix_watchlist_events_target_created_at", "watchlist_target_id", text("created_at DESC")),
        Index("ix_watchlist_events_watchlist_created_at", "watchlist_id", text("created_at DESC"), text("id DESC")),
        Index("ix_watchlist_events_watchlist_is_read_created", "watchlist_id", "is_read", text("created_at DESC")),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    watchlist_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("watchlists.id", ondelete="CASCADE"), nullable=False)
    watchlist_target_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("watchlist_targets.id", ondelete="CASCADE"), nullable=False)
    event_type: Mapped[WatchlistEventType] = mapped_column(Enum(WatchlistEventType, native_enum=False), nullable=False)
    event_payload: Mapped[dict] = mapped_column(JSON, default=dict)
    is_read: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False, server_default="false")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    watchlist: Mapped["Watchlist"] = relationship(back_populates="events")
    target: Mapped["WatchlistTarget"] = relationship(back_populates="events")

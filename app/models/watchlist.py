"""Watchlist model — saved product preferences per user."""
import uuid
from datetime import datetime, UTC

from sqlalchemy import ForeignKey, String, DateTime, Index, text
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base


class Watchlist(Base):
    __tablename__ = "watchlists"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    user_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("users.id"), nullable=False)
    name: Mapped[str] = mapped_column(String(100), nullable=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    entries: Mapped[list["WatchlistEntry"]] = relationship(back_populates="watchlist", cascade="all, delete-orphan")


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
    watchlist_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("watchlists.id"), nullable=False)
    product_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("products.id"), nullable=False)
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    watchlist: Mapped["Watchlist"] = relationship(back_populates="entries")

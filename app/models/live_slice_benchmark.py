import uuid
from datetime import datetime

from sqlalchemy import DateTime, Enum, ForeignKey, Integer, Numeric, String, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.database import Base
from app.models.orderbook import OrderSide


class LiveSliceBenchmark(Base):
    __tablename__ = "live_slice_benchmarks"
    __table_args__ = (
        UniqueConstraint(
            "side",
            "market_product",
            "delivery_point_id",
            "availability_window",
            name="uq_live_slice_benchmarks_slice_key",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    side: Mapped[OrderSide] = mapped_column(Enum(OrderSide, native_enum=False), nullable=False)
    market_product: Mapped[str] = mapped_column(String(64), nullable=False)
    delivery_point_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=False
    )
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False)
    benchmark_price_per_mt_usd: Mapped[float] = mapped_column(Numeric(10, 2), nullable=False)
    total_remaining_quantity_mt: Mapped[float] = mapped_column(Numeric(12, 2), nullable=False)
    order_count: Mapped[int] = mapped_column(Integer, nullable=False, default=0)
    source: Mapped[str] = mapped_column(String(32), nullable=False, default="live_slice_vwap")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow
    )

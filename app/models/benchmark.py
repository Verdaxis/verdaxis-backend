import uuid
from datetime import datetime

from sqlalchemy import DateTime, ForeignKey, Numeric, String, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.database import Base


class Benchmark(Base):
    __tablename__ = "benchmarks"
    __table_args__ = (
        UniqueConstraint(
            "market_product",
            "delivery_point_id",
            "availability_window",
            name="uq_benchmarks_market_key",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    market_product: Mapped[str] = mapped_column(String(64), nullable=False)
    delivery_point_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=False
    )
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False)
    price_per_mt_usd: Mapped[float] = mapped_column(Numeric(10, 2), nullable=False)
    source: Mapped[str] = mapped_column(String(32), nullable=False, default="manual_override")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow
    )

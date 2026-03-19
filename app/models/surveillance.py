import enum
import uuid
from datetime import datetime
from sqlalchemy import String, DateTime, Boolean, Enum, JSON
from sqlalchemy.orm import Mapped, mapped_column
from sqlalchemy.dialects.postgresql import UUID
from app.database import Base


class SurveillanceType(str, enum.Enum):
    FRONT_RUNNING = "FRONT_RUNNING"
    SPOOFING = "SPOOFING"
    WASH_TRADING = "WASH_TRADING"
    LAYERING = "LAYERING"
    MARKING_THE_CLOSE = "MARKING_THE_CLOSE"


class SurveillanceSeverity(str, enum.Enum):
    LOW = "LOW"
    MEDIUM = "MEDIUM"
    HIGH = "HIGH"
    CRITICAL = "CRITICAL"


class SurveillanceStatus(str, enum.Enum):
    OPEN = "OPEN"
    REVIEWING = "REVIEWING"
    ACTIONED = "ACTIONED"
    CLOSED = "CLOSED"


class SurveillanceEvent(Base):
    __tablename__ = "surveillance_events"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    type: Mapped[SurveillanceType] = mapped_column(Enum(SurveillanceType, native_enum=False), nullable=False)
    severity: Mapped[SurveillanceSeverity] = mapped_column(Enum(SurveillanceSeverity, native_enum=False), nullable=False)
    status: Mapped[SurveillanceStatus] = mapped_column(Enum(SurveillanceStatus, native_enum=False), default=SurveillanceStatus.OPEN)

    participants: Mapped[list | None] = mapped_column(JSON, default=list)  # list of user/org UUIDs
    related_orders: Mapped[list | None] = mapped_column(JSON, default=list)  # list of order UUIDs
    related_trades: Mapped[list | None] = mapped_column(JSON, default=list)  # list of trade UUIDs

    description: Mapped[str | None] = mapped_column(String(1000), nullable=True)
    notes: Mapped[str | None] = mapped_column(String(2000), nullable=True)

    window_start: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    window_end: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)

    auto_detected: Mapped[bool] = mapped_column(Boolean, default=True)

    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow)

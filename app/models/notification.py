from sqlalchemy import String, ForeignKey, Enum, DateTime, Boolean, JSON, Text
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime
from app.model_base import Base

class NotificationType(str, enum.Enum):
    SYSTEM = "SYSTEM"
    ORDER_UPDATE = "ORDER_UPDATE"
    DIRECT_ORDER = "DIRECT_ORDER"
    DIRECT_ORDER_OFFER = "DIRECT_ORDER_OFFER"
    USER_STATUS = "USER_STATUS"
    TRADE_INITIATED = "TRADE_INITIATED"
    TRADE_CONFIRMED = "TRADE_CONFIRMED"
    TRADE_DECLINED = "TRADE_DECLINED"
    TRADE_DELIVERED = "TRADE_DELIVERED"
    TRADE_PAID = "TRADE_PAID"
    MATCH_SUGGESTION = "MATCH_SUGGESTION"
    NEGOTIATION_RECEIVED = "NEGOTIATION_RECEIVED"
    NEGOTIATION_COUNTERED = "NEGOTIATION_COUNTERED"
    NEGOTIATION_ACCEPTED = "NEGOTIATION_ACCEPTED"
    NEGOTIATION_DECLINED = "NEGOTIATION_DECLINED"

class Notification(Base):
    __tablename__ = "notifications"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    recipient_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("users.id"), nullable=False)
    
    type: Mapped[NotificationType] = mapped_column(Enum(NotificationType, native_enum=False, length=30), nullable=False)
    title: Mapped[str] = mapped_column(String, nullable=False)
    message: Mapped[str] = mapped_column(Text, nullable=False)
    
    # JSON data for frontend navigation/context (e.g., { "rfq_id": "..." })
    data: Mapped[dict | None] = mapped_column(JSON, default=dict)
    
    is_read: Mapped[bool] = mapped_column(Boolean, default=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    # Relationships
    recipient = relationship("User", backref="notifications")

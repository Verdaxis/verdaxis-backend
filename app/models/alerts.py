"""Price alert model."""
from uuid import uuid4
from sqlalchemy import Column, String, Numeric, Boolean, DateTime, ForeignKey, func
from sqlalchemy.dialects.postgresql import UUID
from app.database import Base


class PriceAlert(Base):
    __tablename__ = "price_alerts"

    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    org_id = Column(UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False)
    product_id = Column(UUID(as_uuid=True), ForeignKey("products.id"), nullable=False)
    delivery_point_id = Column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)
    direction = Column(String, nullable=False)  # "above" or "below"
    threshold_usd = Column(Numeric(10, 2), nullable=False)
    is_active = Column(Boolean, default=True, nullable=False)
    triggered_at = Column(DateTime(timezone=True), nullable=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())

    def __repr__(self):
        return f"<PriceAlert {self.direction} {self.threshold_usd} for product={self.product_id}>"

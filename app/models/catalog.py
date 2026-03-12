from uuid import uuid4
from sqlalchemy import Column, String, Numeric, Boolean, DateTime, func
from sqlalchemy.dialects.postgresql import UUID
from app.database import Base


class Product(Base):
    __tablename__ = "products"

    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    name = Column(String, unique=True, nullable=False)
    fuel_type = Column(String, nullable=False)
    fuel_grade = Column(String, nullable=False)
    unit = Column(String, default="MT")
    min_lot_size = Column(Numeric(12, 2), default=100)
    spec_description = Column(String, nullable=True)
    is_active = Column(Boolean, default=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())

    def __repr__(self):
        return f"<Product {self.name}>"


class DeliveryPoint(Base):
    __tablename__ = "delivery_points"

    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    name = Column(String, unique=True, nullable=False)
    region = Column(String, nullable=False)
    timezone = Column(String, nullable=True)
    is_active = Column(Boolean, default=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())

    def __repr__(self):
        return f"<DeliveryPoint {self.name} ({self.region})>"

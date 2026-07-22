from uuid import uuid4
from sqlalchemy import Column, String, Numeric, Boolean, DateTime, func
from sqlalchemy.dialects.postgresql import UUID
from app.model_base import Base
from app.market_catalog import derive_market_product


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

    @property
    def market_product(self) -> str | None:
        derived = derive_market_product(self.name, self.fuel_type, self.fuel_grade)
        return derived.value if derived else None

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

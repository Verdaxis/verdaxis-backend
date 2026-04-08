from enum import Enum
from uuid import uuid4
from sqlalchemy import Column, String, Numeric, Boolean, DateTime, func
from sqlalchemy.dialects.postgresql import UUID
from app.database import Base


class MarketProduct(str, Enum):
    BIO_METHANOL = "BIO_METHANOL"
    E_METHANOL = "E_METHANOL"
    BIO_ETHANOL = "BIO_ETHANOL"
    SYNTHETIC_ETHANOL = "SYNTHETIC_ETHANOL"


def derive_market_product(name: str, fuel_type: str, fuel_grade: str) -> MarketProduct | None:
    normalized_name = (name or "").strip().lower()
    normalized_type = (fuel_type or "").strip().lower()
    normalized_grade = (fuel_grade or "").strip().lower()

    if normalized_name in {"bio methanol", "methanol green"}:
        return MarketProduct.BIO_METHANOL
    if normalized_name == "e-methanol":
        return MarketProduct.E_METHANOL
    if normalized_name in {"bio ethanol", "ethanol green"}:
        return MarketProduct.BIO_ETHANOL
    if normalized_name == "synthetic ethanol":
        return MarketProduct.SYNTHETIC_ETHANOL

    if normalized_type == "methanol" and normalized_grade in {"bio", "green"}:
        return MarketProduct.BIO_METHANOL
    if normalized_type == "methanol" and normalized_grade in {"e", "synthetic"}:
        return MarketProduct.E_METHANOL
    if normalized_type == "ethanol" and normalized_grade in {"bio", "green"}:
        return MarketProduct.BIO_ETHANOL
    if normalized_type == "ethanol" and normalized_grade == "synthetic":
        return MarketProduct.SYNTHETIC_ETHANOL

    return None


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

from sqlalchemy import String, ForeignKey, Enum, Numeric, Date, DateTime, Boolean
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime, date
from app.database import Base

class FuelType(str, enum.Enum):
    Methanol = "Methanol"
    Biofuel = "Biofuel"
    LNG = "LNG"
    Ammonia = "Ammonia"
    LSMGO = "LSMGO"

class DirectOrderOfferStatus(str, enum.Enum):
    Draft = "Draft"
    Pending = "Pending"
    Quoted = "Quoted"
    Negotiating = "Negotiating"
    Confirmed = "Confirmed"
    Rejected = "Rejected"
    Completed = "Completed"

class InventoryItem(Base):
    __tablename__ = "inventory_items"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    supplier_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    port_id: Mapped[str | None] = mapped_column(ForeignKey("ports.id"))
    fuel_type: Mapped[FuelType] = mapped_column(Enum(FuelType, native_enum=False), nullable=False)
    product_name: Mapped[str | None] = mapped_column(String)
    
    current_stock_mt: Mapped[float] = mapped_column(Numeric(10, 2), nullable=False)
    incoming_stock_mt: Mapped[float] = mapped_column(Numeric(10, 2), default=0)
    reserved_stock_mt: Mapped[float] = mapped_column(Numeric(10, 2), default=0)
    
    price_per_mt_usd: Mapped[float | None] = mapped_column(Numeric(10, 2))
    energy_density_mj_kg: Mapped[float | None] = mapped_column(Numeric(5, 2))
    is_certified: Mapped[bool] = mapped_column(Boolean, default=False)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    port = relationship("Port", back_populates="inventory_items")
    supplier = relationship("Organization")

class DirectOrder(Base):
    __tablename__ = "direct_orders"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    buyer_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    vessel_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("vessels.id"))
    port_id: Mapped[str | None] = mapped_column(ForeignKey("ports.id"))
    
    fuel_type: Mapped[FuelType] = mapped_column(Enum(FuelType, native_enum=False), nullable=False)
    quantity_mt: Mapped[float] = mapped_column(Numeric(10, 2), nullable=False)
    delivery_window_start: Mapped[date | None] = mapped_column(Date)
    delivery_window_end: Mapped[date | None] = mapped_column(Date)
    
    status: Mapped[DirectOrderOfferStatus] = mapped_column(Enum(DirectOrderOfferStatus, native_enum=False), default=DirectOrderOfferStatus.Pending)
    
    awarded_supplier_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    final_price_usd: Mapped[float | None] = mapped_column(Numeric(12, 2))
    final_price_per_mt: Mapped[float | None] = mapped_column(Numeric(10, 2))
    
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    buyer = relationship("Organization", foreign_keys=[buyer_id])
    supplier = relationship("Organization", foreign_keys=[awarded_supplier_id])
    vessel = relationship("Vessel")
    port = relationship("Port")
    offers = relationship("DirectOrderOffer", back_populates="order")

class DirectOrderOffer(Base):
    __tablename__ = "direct_order_offers"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    direct_order_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("direct_orders.id"), nullable=False)
    supplier_id: Mapped[uuid.UUID] = mapped_column(ForeignKey("organizations.id"), nullable=False)
    
    price_per_mt_usd: Mapped[float] = mapped_column(Numeric(10, 2), nullable=False)
    valid_until: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    terms_and_conditions: Mapped[str | None] = mapped_column(String)
    
    is_accepted: Mapped[bool] = mapped_column(Boolean, default=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    order = relationship("DirectOrder", back_populates="offers")
    supplier = relationship("Organization")

from sqlalchemy import String, ForeignKey, Enum, Numeric, DateTime, Boolean, Text
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime
from app.database import Base

class FuelType(str, enum.Enum):
    Methanol = "Methanol"
    Ethanol = "Ethanol"
    Biofuel = "Biofuel"
    Ammonia = "Ammonia"
    Biomethane = "Biomethane"

class InventoryItem(Base):
    __tablename__ = "inventory_items"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    supplier_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    port_id: Mapped[str | None] = mapped_column(ForeignKey("ports.id"))
    fuel_type: Mapped[FuelType] = mapped_column(Enum(FuelType, native_enum=False, length=16), nullable=False)
    product_name: Mapped[str | None] = mapped_column(String)

    current_stock_mt: Mapped[float] = mapped_column(Numeric(10, 2), nullable=False)
    incoming_stock_mt: Mapped[float] = mapped_column(Numeric(10, 2), default=0)
    reserved_stock_mt: Mapped[float] = mapped_column(Numeric(10, 2), default=0)

    price_per_mt_usd: Mapped[float | None] = mapped_column(Numeric(10, 2))
    energy_density_mj_kg: Mapped[float | None] = mapped_column(Numeric(5, 2))
    is_certified: Mapped[bool] = mapped_column(Boolean, default=False)
    certification_declared: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    certification_scheme: Mapped[str | None] = mapped_column(String(120))
    specification_standard: Mapped[str | None] = mapped_column(String(120))
    msds_available: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    carbon_intensity_gco2_mj: Mapped[float | None] = mapped_column(Numeric(8, 2))
    carbon_intensity_method: Mapped[str | None] = mapped_column(String(120))
    feedstock: Mapped[str | None] = mapped_column(String(255))
    origin: Mapped[str | None] = mapped_column(String(255))
    off_spec: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    off_spec_notes: Mapped[str | None] = mapped_column(Text)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    port = relationship("Port", back_populates="inventory_items")
    supplier = relationship("Organization")

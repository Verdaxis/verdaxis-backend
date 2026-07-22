from sqlalchemy import String, ForeignKey, Enum, Numeric, DateTime, Boolean, Text, Index
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime
from decimal import Decimal
from app.model_base import Base
from app.market_constraints import (
    INVENTORY_NUMERIC_VALUES,
    postgresql_check,
)

class FuelType(str, enum.Enum):
    Methanol = "Methanol"
    Ethanol = "Ethanol"
    Biofuel = "Biofuel"
    LNG = "LNG"
    Ammonia = "Ammonia"
    LSMGO = "LSMGO"
    Biomethane = "Biomethane"

class InventoryItem(Base):
    __tablename__ = "inventory_items"
    __table_args__ = (
        Index("ix_inventory_items_supplier_id", "supplier_id"),
        Index("ix_inventory_items_owner_user_id", "owner_user_id"),
        postgresql_check(INVENTORY_NUMERIC_VALUES, name="ck_inventory_items_numeric_values"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    supplier_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    owner_user_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("users.id", ondelete="SET NULL"), nullable=True)
    port_id: Mapped[str | None] = mapped_column(ForeignKey("ports.id"))
    # sec_20260720_boundaries widened storage to VARCHAR(20); the enum length
    # must match the deployed column or autogenerate reports typmod drift.
    fuel_type: Mapped[FuelType] = mapped_column(Enum(FuelType, native_enum=False, length=20), nullable=False)
    product_name: Mapped[str | None] = mapped_column(String)

    current_stock_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)
    incoming_stock_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False, default=Decimal("0"), server_default="0")
    reserved_stock_mt: Mapped[Decimal] = mapped_column(Numeric(), nullable=False, default=Decimal("0"), server_default="0")

    price_per_mt_usd: Mapped[Decimal | None] = mapped_column(Numeric())
    energy_density_mj_kg: Mapped[Decimal | None] = mapped_column(Numeric())
    is_certified: Mapped[bool] = mapped_column(Boolean, default=False)
    certification_declared: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    certification_scheme: Mapped[str | None] = mapped_column(String(120))
    specification_standard: Mapped[str | None] = mapped_column(String(120))
    msds_available: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    carbon_intensity_gco2_mj: Mapped[Decimal | None] = mapped_column(Numeric())
    carbon_intensity_method: Mapped[str | None] = mapped_column(String(120))
    feedstock: Mapped[str | None] = mapped_column(String(255))
    origin: Mapped[str | None] = mapped_column(String(255))
    off_spec: Mapped[bool] = mapped_column(Boolean, default=False, server_default="false")
    off_spec_notes: Mapped[str | None] = mapped_column(Text)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    port = relationship("Port", back_populates="inventory_items")
    supplier = relationship("Organization")

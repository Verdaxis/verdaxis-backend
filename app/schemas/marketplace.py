from pydantic import BaseModel, ConfigDict, Field, field_validator
from typing import Optional
from enum import Enum
from datetime import datetime
from uuid import UUID
from decimal import Decimal

from app.schemas.market_integrity import finite_decimal

class FuelType(str, Enum):
    Methanol = "Methanol"
    Ethanol = "Ethanol"
    Biofuel = "Biofuel"
    LNG = "LNG"
    Ammonia = "Ammonia"
    LSMGO = "LSMGO"

# Inventory Schemas
class InventoryBase(BaseModel):
    model_config = ConfigDict(extra="forbid")

    port_id: str
    fuel_type: FuelType
    product_name: Optional[str] = None
    current_stock_mt: Decimal = Field(ge=0, le=100000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    incoming_stock_mt: Decimal = Field(Decimal("0"), ge=0, le=100000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    price_per_mt_usd: Optional[Decimal] = Field(None, gt=0, le=1000000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    energy_density_mj_kg: Optional[Decimal] = Field(None, ge=0, le=10000, max_digits=5, decimal_places=2, allow_inf_nan=False)
    is_certified: bool = False
    certification_declared: bool = False
    certification_scheme: Optional[str] = None
    specification_standard: Optional[str] = None
    msds_available: bool = False
    carbon_intensity_gco2_mj: Optional[Decimal] = Field(None, ge=0, le=10000, max_digits=8, decimal_places=2, allow_inf_nan=False)
    carbon_intensity_method: Optional[str] = None
    feedstock: Optional[str] = None
    origin: Optional[str] = None
    off_spec: bool = False
    off_spec_notes: Optional[str] = None

    @field_validator("current_stock_mt", "incoming_stock_mt", "price_per_mt_usd", "energy_density_mj_kg", "carbon_intensity_gco2_mj")
    @classmethod
    def _finite_inventory_values(cls, value: Decimal | None, info):
        return None if value is None else finite_decimal(value, field_name=info.field_name)

class InventoryCreate(InventoryBase):
    pass

class InventoryItemUpdate(BaseModel):
    model_config = ConfigDict(extra="forbid")

    product_name: Optional[str] = None
    current_stock_mt: Optional[Decimal] = Field(None, ge=0, le=100000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    incoming_stock_mt: Optional[Decimal] = Field(None, ge=0, le=100000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    price_per_mt_usd: Optional[Decimal] = Field(None, gt=0, le=1000000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    certification_declared: Optional[bool] = None
    certification_scheme: Optional[str] = None
    specification_standard: Optional[str] = None
    msds_available: Optional[bool] = None
    carbon_intensity_gco2_mj: Optional[Decimal] = Field(None, ge=0, le=10000, max_digits=8, decimal_places=2, allow_inf_nan=False)
    carbon_intensity_method: Optional[str] = None
    feedstock: Optional[str] = None
    origin: Optional[str] = None
    off_spec: Optional[bool] = None
    off_spec_notes: Optional[str] = None

    @field_validator("current_stock_mt", "incoming_stock_mt", "price_per_mt_usd", "carbon_intensity_gco2_mj")
    @classmethod
    def _finite_inventory_updates(cls, value: Decimal | None, info):
        return None if value is None else finite_decimal(value, field_name=info.field_name)

class InventoryResponse(InventoryBase):
    model_config = ConfigDict(from_attributes=True, extra="forbid")

    id: UUID
    supplier_id: UUID
    reserved_stock_mt: Decimal = Field(ge=0, le=100000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    updated_at: datetime

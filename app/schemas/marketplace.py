from pydantic import BaseModel
from typing import Optional
from enum import Enum
from datetime import datetime
from uuid import UUID

class FuelType(str, Enum):
    Methanol = "Methanol"
    Ethanol = "Ethanol"
    Biofuel = "Biofuel"
    LNG = "LNG"
    Ammonia = "Ammonia"
    LSMGO = "LSMGO"

# Inventory Schemas
class InventoryBase(BaseModel):
    port_id: str
    fuel_type: FuelType
    product_name: Optional[str] = None
    current_stock_mt: float
    incoming_stock_mt: float = 0
    reserved_stock_mt: float = 0
    price_per_mt_usd: Optional[float] = None
    energy_density_mj_kg: Optional[float] = None
    is_certified: bool = False
    certification_declared: bool = False
    certification_scheme: Optional[str] = None
    specification_standard: Optional[str] = None
    msds_available: bool = False
    carbon_intensity_gco2_mj: Optional[float] = None
    carbon_intensity_method: Optional[str] = None
    feedstock: Optional[str] = None
    origin: Optional[str] = None
    off_spec: bool = False
    off_spec_notes: Optional[str] = None

class InventoryCreate(InventoryBase):
    pass

class InventoryItemUpdate(BaseModel):
    product_name: Optional[str] = None
    current_stock_mt: Optional[float] = None
    incoming_stock_mt: Optional[float] = None
    price_per_mt_usd: Optional[float] = None
    certification_declared: Optional[bool] = None
    certification_scheme: Optional[str] = None
    specification_standard: Optional[str] = None
    msds_available: Optional[bool] = None
    carbon_intensity_gco2_mj: Optional[float] = None
    carbon_intensity_method: Optional[str] = None
    feedstock: Optional[str] = None
    origin: Optional[str] = None
    off_spec: Optional[bool] = None
    off_spec_notes: Optional[str] = None

class InventoryResponse(InventoryBase):
    id: UUID
    supplier_id: UUID
    updated_at: datetime

    class Config:
        from_attributes = True

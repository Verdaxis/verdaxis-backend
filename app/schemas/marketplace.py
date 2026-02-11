from pydantic import BaseModel
from typing import Optional
from enum import Enum
from datetime import datetime
from uuid import UUID

class FuelType(str, Enum):
    Methanol = "Methanol"
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

class InventoryCreate(InventoryBase):
    pass

class InventoryItemUpdate(BaseModel):
    product_name: Optional[str] = None
    current_stock_mt: Optional[float] = None
    incoming_stock_mt: Optional[float] = None
    price_per_mt_usd: Optional[float] = None

class InventoryResponse(InventoryBase):
    id: UUID
    supplier_id: UUID
    updated_at: datetime

    class Config:
        from_attributes = True

from pydantic import BaseModel, condecimal
from typing import Optional, List
from enum import Enum
from datetime import datetime, date
from uuid import UUID

class FuelType(str, Enum):
    Methanol = "Methanol"
    Biofuel = "Biofuel"
    LNG = "LNG"
    Ammonia = "Ammonia"
    LSMGO = "LSMGO"

class DirectOrderOfferStatus(str, Enum):
    Draft = "Draft"
    Pending = "Pending"
    Quoted = "Quoted"
    Negotiating = "Negotiating"
    Confirmed = "Confirmed"
    Rejected = "Rejected"
    Completed = "Completed"

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

class InventoryResponse(InventoryBase):
    id: UUID
    supplier_id: UUID
    updated_at: datetime
    
    class Config:
        from_attributes = True

# Quote Schemas
class DirectOrderBase(BaseModel):
    vessel_id: UUID
    port_id: str
    fuel_type: FuelType
    quantity_mt: float
    delivery_window_start: Optional[date] = None
    delivery_window_end: Optional[date] = None

class DirectOrderCreate(DirectOrderBase):
    pass

class DirectOrderUpdate(BaseModel):
    status: Optional[DirectOrderOfferStatus] = None
    final_price_usd: Optional[float] = None
    final_price_per_mt: Optional[float] = None
    awarded_supplier_id: Optional[UUID] = None

class DirectOrderOfferBase(BaseModel):
    price_per_mt_usd: float
    valid_until: Optional[datetime] = None
    terms_and_conditions: Optional[str] = None

class DirectOrderOfferCreate(DirectOrderOfferBase):
    pass

class DirectOrderOfferResponse(DirectOrderOfferBase):
    id: UUID
    direct_order_id: UUID
    supplier_id: UUID
    is_accepted: bool
    created_at: datetime
    
    class Config:
        from_attributes = True

class DirectOrderResponse(DirectOrderBase):
    id: UUID
    buyer_id: Optional[UUID] = None
    status: DirectOrderOfferStatus
    
    awarded_supplier_id: Optional[UUID] = None
    final_price_usd: Optional[float] = None
    final_price_per_mt: Optional[float] = None
    created_at: datetime
    updated_at: datetime
    
    offers: List[DirectOrderOfferResponse] = []

    class Config:
        from_attributes = True

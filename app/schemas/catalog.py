from pydantic import BaseModel
from typing import Optional
from uuid import UUID
from decimal import Decimal


class ProductResponse(BaseModel):
    id: UUID
    name: str
    market_product: Optional[str] = None
    fuel_type: str
    fuel_grade: str
    unit: str
    min_lot_size: Decimal
    is_active: bool

    model_config = {"from_attributes": True}


class DeliveryPointResponse(BaseModel):
    id: UUID
    name: str
    region: str
    timezone: Optional[str] = None
    is_active: bool

    model_config = {"from_attributes": True}

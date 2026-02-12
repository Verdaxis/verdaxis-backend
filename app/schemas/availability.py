from pydantic import BaseModel
from typing import Optional
from decimal import Decimal
from enum import Enum


class AvailabilityLevel(str, Enum):
    AVAILABLE = "AVAILABLE"       # Green: >1000 MT total stock
    LIMITED = "LIMITED"           # Yellow: 1-1000 MT total stock
    NONE = "NONE"                # Red: 0 MT or no suppliers


class PortFuelAvailability(BaseModel):
    port_id: str
    port_name: str
    lat: float
    lng: float
    fuel_type: str
    total_stock_mt: Decimal
    supplier_count: int
    availability_level: AvailabilityLevel
    avg_price_per_mt: Optional[Decimal] = None

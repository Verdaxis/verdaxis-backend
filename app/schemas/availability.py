from pydantic import BaseModel
from typing import Optional
from decimal import Decimal
from enum import Enum

from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind


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
    market_product_code: str
    total_stock_mt: Optional[Decimal]
    supplier_count: int
    availability_level: AvailabilityLevel
    avg_price_per_mt: Optional[Decimal] = None
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    scope: MarketScope = MarketScope.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN
    unknown_count: int = 0

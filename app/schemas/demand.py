from pydantic import BaseModel
from decimal import Decimal
from datetime import datetime
from enum import Enum
from uuid import UUID


class UrgencyLevel(str, Enum):
    HIGH = "HIGH"       # Spot or delivery within 30 days
    MEDIUM = "MEDIUM"   # Delivery within 90 days
    LOW = "LOW"         # Forward delivery > 90 days


class DemandSignal(BaseModel):
    """
    Anonymized demand signal derived from buyer bid orders.
    No buyer identity is exposed.
    """
    fuel_type: str
    region: str
    market_product_code: str | None = None
    delivery_point_id: UUID | None = None
    delivery_point_name: str | None = None
    availability_window_code: str | None = None
    volume_mt: Decimal
    max_price_per_mt: Decimal
    urgency: UrgencyLevel
    bid_count: int
    earliest_delivery: str  # e.g. "Spot", "Q1 2026", "Forward 2027"
    created_at: datetime

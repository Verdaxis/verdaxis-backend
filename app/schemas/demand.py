from pydantic import BaseModel
from typing import Optional
from decimal import Decimal
from datetime import datetime
from enum import Enum


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
    volume_mt: Decimal
    max_price_per_mt: Decimal
    urgency: UrgencyLevel
    bid_count: int
    earliest_delivery: str  # e.g. "Spot", "Q1 2026", "Forward 2027"
    created_at: datetime

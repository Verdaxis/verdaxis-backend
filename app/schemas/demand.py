from pydantic import BaseModel
from decimal import Decimal
from datetime import datetime
from enum import Enum
from uuid import UUID
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind


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
    # Quarantined UNKNOWN summaries deliberately carry no economic values.
    volume_mt: Decimal | None
    max_price_per_mt: Decimal | None
    urgency: UrgencyLevel
    bid_count: int
    earliest_delivery: str  # e.g. "Spot", "Q1 2026", "Forward 2027"
    created_at: datetime
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    scope: MarketScope = MarketScope.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN
    unknown_count: int = 0

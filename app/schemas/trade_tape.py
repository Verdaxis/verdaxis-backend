"""Schemas for the public trade tape (anonymized confirmed trades)."""

from datetime import datetime
from decimal import Decimal
from typing import Literal, Optional
from uuid import UUID

from pydantic import BaseModel
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind


class TradeTapeEntry(BaseModel):
    id: str  # shortened UUID (first 8 chars for anonymity)
    product_id: Optional[UUID] = None
    market_product: Optional[str] = None
    fuel_type: str
    fuel_grade: str
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    region: str
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    total_usd: Decimal
    confirmed_at: datetime
    availability_window: str
    is_demo_trade: bool = False
    scope: Literal["DELIVERY_POINT", "REGION", "UNKNOWN"] = "UNKNOWN"
    provenance_kind: str = "UNKNOWN"
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN


class TradeTapeResponse(BaseModel):
    items: list[TradeTapeEntry]
    total: int
    market_hours: bool  # true while Verdaxis presents confirmed trades without session delay

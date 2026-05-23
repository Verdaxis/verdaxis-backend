"""Schemas for the public trade tape (anonymized confirmed trades)."""

from datetime import datetime
from decimal import Decimal
from typing import Optional

from pydantic import BaseModel


class TradeTapeEntry(BaseModel):
    id: str  # shortened UUID (first 8 chars for anonymity)
    market_product: Optional[str] = None
    fuel_type: str
    fuel_grade: str
    region: str
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    total_usd: Decimal
    confirmed_at: datetime
    availability_window: str


class TradeTapeResponse(BaseModel):
    items: list[TradeTapeEntry]
    total: int
    market_hours: bool  # true while Verdaxis presents confirmed trades without session delay

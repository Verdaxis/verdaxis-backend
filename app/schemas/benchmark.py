from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID

from pydantic import BaseModel


class BenchmarkQuote(BaseModel):
    market_product: str
    delivery_point_id: UUID
    delivery_point_name: str
    availability_window: str
    benchmark_price_per_mt_usd: Decimal
    source: str
    generated_at: datetime
    observed_at: Optional[datetime] = None


class BenchmarkQuoteResponse(BaseModel):
    items: list[BenchmarkQuote]
    generated_at: datetime


class BenchmarkedPrice(BaseModel):
    benchmark_price_per_mt_usd: Optional[Decimal] = None
    premium_discount_per_mt_usd: Optional[Decimal] = None
    benchmark_source: Optional[str] = None

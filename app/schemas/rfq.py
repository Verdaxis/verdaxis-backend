"""Pydantic schemas for RFQ endpoints."""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator

from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window
from app.schemas.market_integrity import finite_decimal


class RFQCreateRequest(BaseModel):
    product_id: UUID
    delivery_point_id: UUID
    quantity_mt: Decimal = Field(gt=0, le=100000, max_digits=12, decimal_places=2, allow_inf_nan=False)
    target_price_per_mt: Optional[Decimal] = Field(None, gt=0, le=1000000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    availability_window: str = SPOT_WINDOW
    notes: Optional[str] = Field(None, max_length=500)
    is_anonymous: bool = False
    expires_in_hours: int = Field(default=24, ge=1, le=168)

    @field_validator("availability_window", mode="before")
    @classmethod
    def _normalize_availability_window(cls, value: str) -> str:
        return normalize_availability_window(value)

    @field_validator("quantity_mt", "target_price_per_mt")
    @classmethod
    def _finite_values(cls, value: Decimal | None, info):
        return None if value is None else finite_decimal(value, field_name=info.field_name)
class RFQQuoteRequest(BaseModel):
    price_per_mt_usd: Decimal = Field(gt=0, le=1000000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    notes: Optional[str] = Field(None, max_length=500)

    @field_validator("price_per_mt_usd")
    @classmethod
    def _finite_price(cls, value: Decimal):
        return finite_decimal(value, field_name="price_per_mt_usd")
class RFQQuoteResponse(BaseModel):
    id: UUID
    seller_org_id: UUID
    seller_org_name: Optional[str] = None
    price_per_mt_usd: Decimal
    notes: Optional[str] = None
    status: str
    created_at: datetime

    model_config = ConfigDict(from_attributes=True)

class RFQResponse(BaseModel):
    id: UUID
    buyer_org_id: Optional[UUID] = None
    buyer_org_name: Optional[str] = None
    product_id: UUID
    product_name: Optional[str] = None
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    quantity_mt: Decimal
    target_price_per_mt: Optional[Decimal] = None
    availability_window: str
    notes: Optional[str] = None
    is_anonymous: bool
    status: str
    expires_at: datetime
    created_at: datetime
    quote_count: int = 0
    quotes: list[RFQQuoteResponse] = []

    model_config = ConfigDict(from_attributes=True)

class RFQListResponse(BaseModel):
    items: list[RFQResponse]
    total: int

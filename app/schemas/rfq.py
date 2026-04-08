"""Pydantic schemas for RFQ endpoints."""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator

from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window


class RFQCreateRequest(BaseModel):
    product_id: UUID
    delivery_point_id: Optional[UUID] = None
    quantity_mt: Decimal = Field(gt=0, le=100000)
    target_price_per_mt: Optional[Decimal] = Field(None, gt=0)
    availability_window: str = SPOT_WINDOW
    notes: Optional[str] = Field(None, max_length=500)
    is_anonymous: bool = False
    expires_in_hours: int = Field(default=24, ge=1, le=168)

    @field_validator("availability_window", mode="before")
    @classmethod
    def _normalize_availability_window(cls, value: str) -> str:
        return normalize_availability_window(value)
class RFQQuoteRequest(BaseModel):
    price_per_mt_usd: Decimal = Field(gt=0)
    notes: Optional[str] = Field(None, max_length=500)
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
    buyer_org_id: UUID
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

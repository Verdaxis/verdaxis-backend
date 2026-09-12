"""Pydantic schemas for subscription endpoints."""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, Field

from app.models.subscription import SubscriptionTier


class SubscriptionResponse(BaseModel):
    id: UUID
    org_id: UUID
    tier: SubscriptionTier
    started_at: Optional[datetime] = None
    expires_at: Optional[datetime] = None
    is_active: bool
    seller_fee_per_mt_usd: Optional[Decimal] = None

    model_config = {"from_attributes": True}


class SubscriptionUpdate(BaseModel):
    tier: SubscriptionTier
    seller_fee_per_mt_usd: Optional[Decimal] = Field(
        default=None,
        ge=0,
        le=100000,
        decimal_places=2,
        allow_inf_nan=False,
    )


class FeeScheduleResponse(BaseModel):
    currency: str = "USD"
    buyer_fee_per_mt_usd: Decimal = Decimal("0")
    seller_fee_per_mt_usd: dict[SubscriptionTier, Optional[Decimal]]

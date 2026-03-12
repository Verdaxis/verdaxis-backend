"""Pydantic schemas for price alerts."""
from pydantic import BaseModel, field_validator
from uuid import UUID
from decimal import Decimal
from datetime import datetime
from typing import Optional


class AlertCreate(BaseModel):
    product_id: UUID
    delivery_point_id: Optional[UUID] = None
    direction: str  # "above" or "below"
    threshold_usd: Decimal

    @field_validator("direction")
    @classmethod
    def validate_direction(cls, v: str) -> str:
        if v not in ("above", "below"):
            raise ValueError("direction must be 'above' or 'below'")
        return v

    @field_validator("threshold_usd")
    @classmethod
    def validate_threshold(cls, v: Decimal) -> Decimal:
        if v <= 0:
            raise ValueError("threshold_usd must be positive")
        return v


class AlertResponse(BaseModel):
    id: UUID
    org_id: UUID
    product_id: UUID
    delivery_point_id: Optional[UUID]
    direction: str
    threshold_usd: Decimal
    is_active: bool
    triggered_at: Optional[datetime]
    created_at: datetime

    model_config = {"from_attributes": True}

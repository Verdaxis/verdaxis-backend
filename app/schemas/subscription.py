"""Pydantic schemas for subscription endpoints."""
from datetime import datetime
from typing import Optional
from uuid import UUID

from pydantic import BaseModel

from app.models.subscription import SubscriptionTier


class SubscriptionResponse(BaseModel):
    id: UUID
    org_id: UUID
    tier: SubscriptionTier
    started_at: Optional[datetime] = None
    expires_at: Optional[datetime] = None
    is_active: bool

    model_config = {"from_attributes": True}


class SubscriptionUpdate(BaseModel):
    tier: SubscriptionTier

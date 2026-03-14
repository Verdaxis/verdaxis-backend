"""Pydantic schemas for referral endpoints."""
from datetime import datetime
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, EmailStr


class ReferralCodeResponse(BaseModel):
    referral_code: str
    referral_link: str


class ReferralInviteRequest(BaseModel):
    email: EmailStr


class ReferralListItem(BaseModel):
    organization_name: Optional[str] = None
    role: Optional[str] = None
    status: str
    signed_up_at: datetime

    model_config = {"from_attributes": True}  # replaces class Config


class ReferralStatsResponse(BaseModel):
    total: int
    verified: int
    active: int
    referrals: list[ReferralListItem]


class LeaderboardEntry(BaseModel):
    rank: int
    user_name: str
    organization_name: Optional[str] = None
    referral_count: int


class ResolveCodeResponse(BaseModel):
    valid: bool
    organization_name: Optional[str] = None
    organization_type: Optional[str] = None
    referrer_name: Optional[str] = None

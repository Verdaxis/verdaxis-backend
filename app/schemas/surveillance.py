from pydantic import BaseModel
from typing import Optional
from uuid import UUID
from datetime import datetime
from enum import Enum


# Enums duplicated per project convention (not shared with model layer)
class SurveillanceType(str, Enum):
    FRONT_RUNNING = "FRONT_RUNNING"
    SPOOFING = "SPOOFING"
    WASH_TRADING = "WASH_TRADING"
    LAYERING = "LAYERING"
    MARKING_THE_CLOSE = "MARKING_THE_CLOSE"


class SurveillanceSeverity(str, Enum):
    LOW = "LOW"
    MEDIUM = "MEDIUM"
    HIGH = "HIGH"
    CRITICAL = "CRITICAL"


class SurveillanceStatus(str, Enum):
    OPEN = "OPEN"
    REVIEWING = "REVIEWING"
    ACTIONED = "ACTIONED"
    CLOSED = "CLOSED"


class SurveillanceEventResponse(BaseModel):
    id: UUID
    type: SurveillanceType
    severity: SurveillanceSeverity
    status: SurveillanceStatus
    participants: Optional[list] = None
    related_orders: Optional[list] = None
    related_trades: Optional[list] = None
    description: Optional[str] = None
    notes: Optional[str] = None
    window_start: Optional[datetime] = None
    window_end: Optional[datetime] = None
    auto_detected: bool
    created_at: datetime
    updated_at: datetime

    class Config:
        from_attributes = True


class SurveillanceEventUpdate(BaseModel):
    """Only status and notes may be updated by reviewers."""
    status: Optional[SurveillanceStatus] = None
    notes: Optional[str] = None

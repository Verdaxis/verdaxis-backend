from pydantic import BaseModel
from typing import Optional
from uuid import UUID
from datetime import datetime, date
from decimal import Decimal
from enum import Enum


class CommissionStatus(str, Enum):
    PENDING = "PENDING"
    INVOICED = "INVOICED"
    PAID = "PAID"


# ============== Commission Schemas ==============

class CommissionResponse(BaseModel):
    id: UUID
    match_id: UUID
    amount_usd: Decimal
    status: CommissionStatus
    invoice_number: Optional[str] = None
    invoice_date: Optional[date] = None
    payment_date: Optional[date] = None
    created_at: datetime

    class Config:
        from_attributes = True


class CommissionUpdate(BaseModel):
    status: Optional[CommissionStatus] = None
    invoice_number: Optional[str] = None
    invoice_date: Optional[date] = None
    payment_date: Optional[date] = None
    notes: Optional[str] = None


# ============== Admin Dashboard Schemas ==============

class CommissionSummary(BaseModel):
    """Summary stats for admin dashboard."""
    total_pending_usd: Decimal
    total_invoiced_usd: Decimal
    total_paid_usd: Decimal
    pending_count: int
    invoiced_count: int
    paid_count: int

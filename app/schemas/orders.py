from pydantic import BaseModel, Field
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
    total_pending_usd: Decimal = Field(description="Sum of commission amounts in PENDING status — accrued but not yet invoiced.")
    total_invoiced_usd: Decimal = Field(description="Sum of commission amounts in INVOICED status — billed but not yet collected.")
    total_paid_usd: Decimal = Field(description="Sum of commission amounts in PAID status — collected revenue.")
    pending_count: int = Field(description="Number of commissions in PENDING status.")
    invoiced_count: int = Field(description="Number of commissions in INVOICED status.")
    paid_count: int = Field(description="Number of commissions in PAID status.")

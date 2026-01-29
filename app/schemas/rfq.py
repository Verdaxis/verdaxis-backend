from pydantic import BaseModel, Field
from typing import Optional
from uuid import UUID
from datetime import datetime, date
from decimal import Decimal
from enum import Enum


# Enums matching SQLAlchemy models
class ListingStatus(str, Enum):
    ACTIVE = "ACTIVE"
    INACTIVE = "INACTIVE"
    EXPIRED = "EXPIRED"


class MatchStatus(str, Enum):
    PENDING = "PENDING"
    ACCEPTED = "ACCEPTED"
    DECLINED = "DECLINED"
    COMPLETED = "COMPLETED"
    CANCELLED = "CANCELLED"


class CommissionStatus(str, Enum):
    PENDING = "PENDING"
    INVOICED = "INVOICED"
    PAID = "PAID"


class FuelGrade(str, Enum):
    CONVENTIONAL = "Conventional"
    GREEN = "Green"
    BIO = "Bio"


class AvailabilityWindow(str, Enum):
    SPOT = "Spot"
    Q1_2025 = "Q1 2025"
    Q2_2025 = "Q2 2025"
    Q3_2025 = "Q3 2025"
    Q4_2025 = "Q4 2025"
    Q1_2026 = "Q1 2026"
    Q2_2026 = "Q2 2026"
    Q3_2026 = "Q3 2026"
    Q4_2026 = "Q4 2026"
    FORWARD_2027 = "Forward 2027"
    FORWARD_2028 = "Forward 2028"


class TierLabel(str, Enum):
    TIER_1_PRODUCER = "Tier 1 Producer"
    MAJOR_TRADER = "Major Trader"
    REGIONAL_SUPPLIER = "Regional Supplier"
    INDEPENDENT = "Independent Supplier"


# ============== Public Listing Schemas ==============

class PublicListingBase(BaseModel):
    region: str = Field(..., min_length=1, max_length=50)
    fuel_type: str = Field(..., min_length=1, max_length=50)
    fuel_grade: FuelGrade = FuelGrade.CONVENTIONAL
    quantity_mt: Decimal = Field(..., gt=0)
    price_per_mt_usd: Decimal = Field(..., gt=0)
    availability_window: AvailabilityWindow = AvailabilityWindow.SPOT
    certifications: list[str] = []


class PublicListingCreate(PublicListingBase):
    """Used by suppliers to create a new listing."""
    tier_label: TierLabel = TierLabel.REGIONAL_SUPPLIER


class PublicListingUpdate(BaseModel):
    """Used by suppliers to update their listing."""
    quantity_mt: Optional[Decimal] = None
    price_per_mt_usd: Optional[Decimal] = None
    availability_window: Optional[AvailabilityWindow] = None
    status: Optional[ListingStatus] = None
    certifications: Optional[list[str]] = None


class PublicListingResponse(BaseModel):
    """
    Anonymized listing response for buyers.
    Note: supplier_id is NOT included to maintain anonymity.
    """
    id: UUID
    region: str
    fuel_type: str
    fuel_grade: FuelGrade
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    availability_window: AvailabilityWindow
    tier_label: TierLabel
    certifications: list[str]
    is_verdaxis_verified: bool
    status: ListingStatus
    created_at: datetime

    class Config:
        from_attributes = True


class PublicListingSupplierResponse(PublicListingResponse):
    """
    Full listing response for the supplier who owns it.
    Includes supplier_id and match count.
    """
    supplier_id: UUID
    match_count: int = 0


class AggregatedListingResponse(BaseModel):
    """
    Aggregated view for buyers: filtered by region and fuel type.
    """
    region: str
    fuel_type: str
    min_price: Decimal
    max_price: Decimal
    total_quantity: Decimal
    listing_count: int


# ============== RFQ Match Schemas ==============

class RFQRequestCreate(BaseModel):
    """Buyer sends this to request a quote on a listing."""
    listing_id: UUID
    accepted_terms: bool = Field(..., description="Must be true to proceed")


class RFQMatchResponse(BaseModel):
    """Response after creating an RFQ match."""
    id: UUID
    listing_id: UUID
    buyer_id: UUID
    status: MatchStatus
    buyer_accepted_terms_at: datetime
    created_at: datetime

    class Config:
        from_attributes = True


class RFQMatchDetailResponse(RFQMatchResponse):
    """
    Detailed match response with de-anonymized info.
    Only shown to the matched parties.
    """
    # Listing details
    region: str
    fuel_type: str
    fuel_grade: FuelGrade
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    
    # Supplier info (de-anonymized)
    supplier_id: UUID
    supplier_name: str
    
    # Buyer info
    buyer_name: str
    
    # Final deal details (if completed)
    final_quantity_mt: Optional[Decimal] = None
    final_price_per_mt: Optional[Decimal] = None
    final_total_usd: Optional[Decimal] = None


class RFQMatchUpdate(BaseModel):
    """Used by supplier to respond to an RFQ."""
    status: MatchStatus


class RFQMatchComplete(BaseModel):
    """Used to complete a match and calculate commission."""
    final_quantity_mt: Decimal = Field(..., gt=0)
    final_price_per_mt: Decimal = Field(..., gt=0)


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

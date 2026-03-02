from pydantic import BaseModel, Field
from typing import Optional
from uuid import UUID
from datetime import datetime, date
from decimal import Decimal
from enum import Enum


# Enums matching SQLAlchemy models
class OrderSide(str, Enum):
    BID = "BID"
    ASK = "ASK"


class OrderBookStatus(str, Enum):
    OPEN = "OPEN"
    PARTIALLY_FILLED = "PARTIALLY_FILLED"
    FILLED = "FILLED"
    CANCELLED = "CANCELLED"
    EXPIRED = "EXPIRED"


class TradeStatus(str, Enum):
    PENDING_CONFIRMATION = "PENDING_CONFIRMATION"
    CONFIRMED = "CONFIRMED"
    DELIVERED = "DELIVERED"
    PAID = "PAID"
    CANCELLED = "CANCELLED"
    DECLINED = "DECLINED"


class Initiator(str, Enum):
    BUYER = "BUYER"
    SELLER = "SELLER"


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
    TIER_1_PRODUCER = "TIER_1_PRODUCER"
    MAJOR_TRADER = "MAJOR_TRADER"
    REGIONAL_SUPPLIER = "REGIONAL_SUPPLIER"
    INDEPENDENT = "INDEPENDENT"


# ============== Order Schemas ==============

class OrderCreate(BaseModel):
    """Used by both buyers (side=BID) and suppliers (side=ASK) to place an order."""
    side: OrderSide
    fuel_type: str = Field(..., min_length=1, max_length=50)
    fuel_grade: FuelGrade = FuelGrade.CONVENTIONAL
    region: str = Field(..., min_length=1, max_length=50)
    port_id: Optional[str] = None
    vessel_id: Optional[UUID] = None
    quantity_mt: Decimal = Field(..., gt=0)
    price_per_mt_usd: Decimal = Field(..., gt=0)
    availability_window: AvailabilityWindow = AvailabilityWindow.SPOT
    delivery_window_start: Optional[date] = None
    delivery_window_end: Optional[date] = None
    certifications: list[str] = Field(default_factory=list)
    expires_at: Optional[datetime] = None


class OrderUpdate(BaseModel):
    """Optional fields for modifying open orders."""
    quantity_mt: Optional[Decimal] = None
    price_per_mt_usd: Optional[Decimal] = None
    availability_window: Optional[AvailabilityWindow] = None
    delivery_window_start: Optional[date] = None
    delivery_window_end: Optional[date] = None
    certifications: Optional[list[str]] = None
    expires_at: Optional[datetime] = None


class OrderResponse(BaseModel):
    """Public/anonymized order for the book. organization_id is NOT included."""
    id: UUID
    side: OrderSide
    fuel_type: str
    fuel_grade: FuelGrade
    region: str
    port_id: Optional[str] = None
    quantity_mt: Decimal
    remaining_quantity_mt: Decimal
    price_per_mt_usd: Decimal
    availability_window: AvailabilityWindow
    delivery_window_start: Optional[date] = None
    delivery_window_end: Optional[date] = None
    certifications: list[str] = Field(default_factory=list)
    is_verdaxis_verified: bool
    tier_label: TierLabel = TierLabel.INDEPENDENT
    status: OrderBookStatus
    expires_at: Optional[datetime] = None
    created_at: datetime

    class Config:
        from_attributes = True


class OrderMyResponse(OrderResponse):
    """Owner view with extra detail (includes org ID and vessel)."""
    organization_id: UUID
    vessel_id: Optional[UUID] = None
    updated_at: datetime
    trade_count: int = 0


# ============== Trade Schemas ==============

class TradeCreate(BaseModel):
    """Hit an order to create a trade."""
    order_id: UUID
    quantity_mt: Decimal = Field(..., gt=0)


class TradeResponse(BaseModel):
    """Full trade detail with de-anonymized party names."""
    id: UUID
    bid_order_id: Optional[UUID] = None
    ask_order_id: Optional[UUID] = None
    buyer_id: UUID
    seller_id: UUID
    buyer_name: str = ""
    seller_name: str = ""
    initiated_by: Initiator
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    status: TradeStatus
    final_quantity_mt: Optional[Decimal] = None
    final_price_per_mt: Optional[Decimal] = None
    final_total_usd: Optional[Decimal] = None
    commission_rate_pct: Decimal = Decimal("0.5")
    commission_amount_usd: Optional[Decimal] = None
    confirmed_at: Optional[datetime] = None
    delivered_at: Optional[datetime] = None
    paid_at: Optional[datetime] = None
    created_at: datetime
    # Denormalized order info for display
    fuel_type: str = ""
    fuel_grade: Optional[FuelGrade] = None
    region: str = ""

    class Config:
        from_attributes = True


class TradeDeliverPayload(BaseModel):
    """Payload for marking a trade as delivered."""
    final_quantity_mt: Decimal = Field(..., gt=0)
    final_price_per_mt: Decimal = Field(..., gt=0)


# ============== Aggregated Market Data ==============

class AggregatedOrderbookResponse(BaseModel):
    """Market data aggregated by region, fuel type, and side."""
    region: str
    fuel_type: str
    side: OrderSide
    min_price: Decimal
    max_price: Decimal
    total_quantity: Decimal
    order_count: int


# ============== Price Discovery ==============

class PriceSummary(BaseModel):
    """Aggregated trade price data for a fuel_type + region pair."""
    fuel_type: str
    region: str
    last_price: Optional[Decimal] = None
    avg_price_24h: Optional[Decimal] = None
    high_24h: Optional[Decimal] = None
    low_24h: Optional[Decimal] = None
    volume_24h: Decimal = Decimal("0")
    trade_count_24h: int = 0
    price_change_pct: Optional[Decimal] = None
    last_trade_at: Optional[datetime] = None


class PriceDiscoveryResponse(BaseModel):
    """Wrapper for multiple price summaries."""
    summaries: list[PriceSummary]
    generated_at: datetime


# ============== Reference Price (VWAP) ==============

class ReferencePriceItem(BaseModel):
    """Daily VWAP reference price for a fuel_type + region pair."""
    fuel_type: str
    region: str
    vwap_usd: Decimal
    total_volume_mt: Decimal
    trade_count: int
    date: date


class ReferencePriceResponse(BaseModel):
    """Wrapper for VWAP reference price data."""
    prices: list[ReferencePriceItem]
    generated_at: datetime


# ============== CI-Adjusted Pricing ==============

class CIAdjustedPrice(BaseModel):
    """
    CI-adjusted effective price for an orderbook order.

    effective_price = base_price + compliance_cost_differential
    where compliance_cost accounts for the carbon intensity gap
    vs FuelEU Maritime reference value (91 gCO2eq/MJ for 2025).
    """
    base_price_per_mt: Decimal
    carbon_intensity_gco2_mj: Decimal
    fueleu_ghg_intensity: Decimal
    compliance_cost_per_mt: Decimal
    effective_price_per_mt: Decimal
    ghg_reduction_pct: Decimal


class OrderResponseWithCI(OrderResponse):
    """OrderResponse enriched with CI-adjusted pricing when CI data is available."""
    ci_adjusted_price: Optional[CIAdjustedPrice] = None

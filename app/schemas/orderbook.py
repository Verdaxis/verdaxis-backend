from pydantic import BaseModel, Field, field_validator, model_validator
from typing import Optional, Literal, Annotated
from uuid import UUID
from datetime import datetime, date
from decimal import Decimal
from enum import Enum

from app.services.availability_windows import (
    JSON_SCHEMA_PATTERN,
    SPOT_WINDOW,
    normalize_availability_window,
)


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
    E = "E"
    SYNTHETIC = "Synthetic"


class TierLabel(str, Enum):
    TIER_1_PRODUCER = "TIER_1_PRODUCER"
    MAJOR_TRADER = "MAJOR_TRADER"
    REGIONAL_SUPPLIER = "REGIONAL_SUPPLIER"
    INDEPENDENT = "INDEPENDENT"


AvailabilityWindowCode = Annotated[
    str,
    Field(
        pattern=JSON_SCHEMA_PATTERN,
        examples=[SPOT_WINDOW, "2026-04", "2026-Q3"],
    ),
]


class AvailabilityWindowMixin(BaseModel):
    @field_validator("availability_window", mode="before", check_fields=False)
    @classmethod
    def _normalize_availability_window(cls, value: str | None):
        if value is None:
            return value
        return normalize_availability_window(value)


class SupplierListingMetadataMixin(BaseModel):
    certification_declared: bool = False
    certification_scheme: Optional[str] = None
    specification_standard: Optional[str] = None
    msds_available: bool = False
    carbon_intensity_gco2_mj: Optional[Decimal] = Field(None, ge=0)
    carbon_intensity_method: Optional[str] = None
    feedstock: Optional[str] = None
    origin: Optional[str] = None
    off_spec: bool = False
    off_spec_notes: Optional[str] = None


# ============== Order Schemas ==============

class OrderCreate(AvailabilityWindowMixin, SupplierListingMetadataMixin):
    """Used by both buyers (side=BID) and suppliers (side=ASK) to place an order."""
    side: OrderSide
    product_id: UUID
    delivery_point_id: UUID
    port_id: Optional[str] = None
    vessel_id: Optional[UUID] = None
    quantity_mt: Decimal = Field(..., gt=0)
    price_per_mt_usd: Decimal = Field(..., gt=0)
    availability_window: AvailabilityWindowCode = SPOT_WINDOW
    certifications: list[str] = Field(default_factory=list)
    certification_scheme: Optional[str] = None
    expires_at: Optional[datetime] = None
    is_anonymous: bool = True

    @model_validator(mode="after")
    def validate_execution_qualifiers(self):
        seen: set[str] = set()
        normalized_certifications: list[str] = []
        for certification in self.certifications:
            normalized = certification.strip()
            if not normalized or normalized in seen:
                continue
            seen.add(normalized)
            normalized_certifications.append(normalized)
        self.certifications = normalized_certifications
        if self.certification_scheme is not None:
            self.certification_scheme = self.certification_scheme.strip() or None
        return self


class OrderUpdate(AvailabilityWindowMixin):
    """Optional fields for modifying open orders."""
    quantity_mt: Optional[Decimal] = Field(None, gt=0)
    price_per_mt_usd: Optional[Decimal] = Field(None, gt=0)
    availability_window: Optional[AvailabilityWindowCode] = None
    certifications: Optional[list[str]] = None
    certification_declared: Optional[bool] = None
    certification_scheme: Optional[str] = None
    specification_standard: Optional[str] = None
    msds_available: Optional[bool] = None
    carbon_intensity_gco2_mj: Optional[Decimal] = Field(None, ge=0)
    carbon_intensity_method: Optional[str] = None
    feedstock: Optional[str] = None
    origin: Optional[str] = None
    off_spec: Optional[bool] = None
    off_spec_notes: Optional[str] = None
    expires_at: Optional[datetime] = None


class OrderResponse(AvailabilityWindowMixin, SupplierListingMetadataMixin):
    """Public/anonymized order for the book. organization_id is NOT included."""
    id: UUID
    side: OrderSide
    product_id: UUID
    product_name: str = ""
    market_product: Optional[str] = None
    fuel_type: str = ""
    fuel_grade: str = ""
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    availability_window: str = "SPOT"
    region: str = ""
    port_id: Optional[str] = None
    quantity_mt: Decimal
    remaining_quantity_mt: Decimal
    price_per_mt_usd: Decimal
    availability_window: AvailabilityWindowCode
    certifications: list[str] = Field(default_factory=list)
    is_verdaxis_verified: bool
    tier_label: TierLabel = TierLabel.INDEPENDENT
    status: OrderBookStatus
    expires_at: Optional[datetime] = None
    created_at: datetime
    benchmark_price_per_mt_usd: Optional[Decimal] = None
    premium_discount_per_mt_usd: Optional[Decimal] = None
    benchmark_source: Optional[str] = None
    is_crossed: bool = False
    is_demo_listing: bool = False

    class Config:
        from_attributes = True


class OrderMyResponse(OrderResponse):
    """Owner view with extra detail (includes org ID and vessel)."""
    organization_id: UUID
    vessel_id: Optional[UUID] = None
    updated_at: datetime
    trade_count: int = 0


class SupplierListingTemplateResponse(AvailabilityWindowMixin, SupplierListingMetadataMixin):
    """Safe supplier defaults for creating the next ASK listing."""
    product_id: UUID
    delivery_point_id: UUID
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    availability_window: AvailabilityWindowCode
    certifications: list[str] = Field(default_factory=list)


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
    is_anonymous: bool = False
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
    product_id: Optional[UUID] = None
    product_name: str = ""
    market_product: Optional[str] = None
    fuel_type: str = ""
    fuel_grade: str = ""
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    availability_window: str = "SPOT"
    region: str = ""

    class Config:
        from_attributes = True


class TradeDeliverPayload(BaseModel):
    """Payload for marking a trade as delivered."""
    final_quantity_mt: Decimal = Field(..., gt=0)
    final_price_per_mt: Decimal = Field(..., gt=0)


# ============== Aggregated Market Data ==============

class AggregatedOrderbookResponse(BaseModel):
    """Market data aggregated by product and delivery point."""
    product_id: UUID
    product_name: str = ""
    fuel_type: str = ""
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    availability_window: str = "SPOT"
    region: str = ""
    side: OrderSide
    min_price: Decimal
    max_price: Decimal
    total_quantity: Decimal
    order_count: int


# ============== Price Discovery ==============

class PriceSummary(BaseModel):
    """Aggregated trade price data for a product + delivery_point + availability window."""
    product_id: Optional[UUID] = None
    product_name: str = ""
    market_product: Optional[str] = None
    fuel_type: str = ""
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    availability_window: str = "SPOT"
    region: str = ""
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
    """Daily VWAP reference price for a product + delivery_point + availability window."""
    product_id: Optional[UUID] = None
    product_name: str = ""
    market_product: Optional[str] = None
    fuel_type: str = ""
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    availability_window: str = "SPOT"
    region: str = ""
    vwap_usd: Decimal
    total_volume_mt: Decimal
    trade_count: int
    date: date
    visibility: Literal["internal", "external"] = "external"


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

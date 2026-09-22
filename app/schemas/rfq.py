"""Pydantic schemas for RFQ endpoints."""
from datetime import datetime
from decimal import Decimal
from typing import Literal, Optional
from uuid import UUID
from zoneinfo import ZoneInfo

from pydantic import AwareDatetime, BaseModel, ConfigDict, Field, field_validator, model_validator

from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window
from app.schemas.market_integrity import finite_decimal
from app.schemas.fame import FameContractTerms, FameOfferTerms
from app.schemas.supplier_offer import SupplierOfferPublicSnapshot, SupplierOfferSnapshot


class RFQCreateRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")

    product_id: UUID
    delivery_point_id: UUID
    quantity_mt: Decimal = Field(gt=0, le=100000, max_digits=12, decimal_places=2, allow_inf_nan=False)
    target_price_per_mt: Optional[Decimal] = Field(None, gt=0, le=1000000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    availability_window: str = SPOT_WINDOW
    notes: Optional[str] = Field(None, max_length=500)
    is_anonymous: bool = False
    expires_in_hours: int = Field(default=24, ge=1, le=168)
    contract_terms: FameContractTerms | None = None
    source_offer_id: UUID | None = None
    expected_source_offer_revision: int | None = Field(None, ge=1)

    @model_validator(mode="after")
    def _validate_minimum_fill(self):
        if (self.source_offer_id is None) != (self.expected_source_offer_revision is None):
            raise ValueError("source_offer_id and expected_source_offer_revision must be supplied together")
        if self.contract_terms and self.contract_terms.min_fill_mt > self.quantity_mt:
            raise ValueError("min_fill_mt must not exceed quantity_mt")
        return self

    @field_validator("availability_window", mode="before")
    @classmethod
    def _normalize_availability_window(cls, value: str) -> str:
        return normalize_availability_window(value)

    @field_validator("quantity_mt", "target_price_per_mt")
    @classmethod
    def _finite_values(cls, value: Decimal | None, info):
        return None if value is None else finite_decimal(value, field_name=info.field_name)


class RFQQuoteRequest(BaseModel):
    price_per_mt_usd: Decimal = Field(gt=0, le=1000000, max_digits=10, decimal_places=2, allow_inf_nan=False)
    notes: Optional[str] = Field(None, max_length=500)
    expires_at: AwareDatetime | None = None
    offer_terms: FameOfferTerms | None = None

    @model_validator(mode="after")
    def _validate_new_declaration(self):
        # Apply new ingress rules here so historical stored quotes still read.
        offer = self.offer_terms
        if offer is None:
            return self
        if offer.standard == "ASTM_D6751" and offer.astm_grade is None:
            raise ValueError("ASTM D6751 quotes require the declared grade")
        if offer.standard != "ASTM_D6751" and offer.astm_grade is not None:
            raise ValueError("ASTM grade requires ASTM D6751 as the standard")
        if offer.quality_evidence:
            today = datetime.now(ZoneInfo("Asia/Singapore")).date()
            for evidence_date in (offer.quality_evidence.sampled_on, offer.quality_evidence.tested_on):
                if evidence_date and evidence_date > today:
                    raise ValueError("Quality evidence sampling and test dates must not be in the future")
        return self

    @field_validator("price_per_mt_usd")
    @classmethod
    def _finite_price(cls, value: Decimal):
        return finite_decimal(value, field_name="price_per_mt_usd")


class RFQQuoteRevisionRequest(RFQQuoteRequest):
    expected_revision: int = Field(ge=1)


class RFQQuoteResponse(BaseModel):
    id: UUID
    seller_org_id: UUID
    seller_org_name: Optional[str] = None
    price_per_mt_usd: Decimal
    notes: Optional[str] = None
    status: str
    created_at: datetime
    expires_at: datetime | None = None
    revision: int = 1
    is_expired: bool = False
    offer_terms: FameOfferTerms | None = None
    execution_enabled: Literal[False] = False

    model_config = ConfigDict(from_attributes=True)


class RFQResponse(BaseModel):
    id: UUID
    buyer_org_id: Optional[UUID] = None
    buyer_org_name: Optional[str] = None
    product_id: UUID
    product_name: Optional[str] = None
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    quantity_mt: Decimal
    target_price_per_mt: Optional[Decimal] = None
    availability_window: str
    notes: Optional[str] = None
    is_anonymous: bool
    status: str
    expires_at: datetime
    created_at: datetime
    quote_count: int = 0
    quotes: list[RFQQuoteResponse] = Field(default_factory=list)
    contract_terms: FameContractTerms | None = None
    source_offer_id: UUID | None = None
    target_supplier_org_id: UUID | None = None
    source_offer_snapshot: SupplierOfferSnapshot | SupplierOfferPublicSnapshot | None = None
    execution_enabled: Literal[False] = False
    can_cancel: bool = False

    model_config = ConfigDict(from_attributes=True)


class RFQListResponse(BaseModel):
    items: list[RFQResponse]
    total: int

"""Typed listing terms and deliberately anonymous marketplace projections."""

from datetime import date, datetime
from decimal import Decimal
from typing import Literal
from uuid import UUID

from pydantic import (
    AwareDatetime,
    BaseModel,
    ConfigDict,
    Field,
    field_validator,
    model_validator,
)

from app.schemas.fame import (
    ASTMGrade,
    FameListingFuelTerms,
    FameQualityResult,
    FuelStandard,
    ShortText,
    SustainabilityScheme,
    TermsText,
)
from app.services.availability_windows import normalize_availability_window


class SupplierOfferCommercialTerms(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)

    delivery_basis: Literal["EX_TANK", "FOB", "FCA", "CIF"]
    named_location: ShortText
    delivery_start: date
    delivery_end: date
    quantity_tolerance_pct: Decimal = Field(ge=0, le=10, allow_inf_nan=False)
    payment_terms: TermsText | None = None
    inspection_terms: TermsText | None = None
    title_risk_terms: TermsText | None = None
    claims_terms: TermsText | None = None
    evidence_due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"]
    notes: str | None = Field(None, max_length=500)


class SupplierOfferTerms(SupplierOfferCommercialTerms):
    fuel_terms: FameListingFuelTerms


class SupplierOfferCreate(SupplierOfferTerms):
    product_id: UUID
    delivery_point_id: UUID
    quantity_mt: Decimal = Field(ge=1, le=100000, decimal_places=2, allow_inf_nan=False)
    min_fill_mt: Decimal = Field(ge=1, le=100000, decimal_places=2, allow_inf_nan=False)
    price_per_mt_usd: Decimal = Field(
        gt=0, le=1000000, decimal_places=2, allow_inf_nan=False
    )
    availability_window: str = "SPOT"
    expires_at: AwareDatetime

    @field_validator("availability_window", mode="before")
    @classmethod
    def normalize_window(cls, value):
        return normalize_availability_window(value)

    @model_validator(mode="after")
    def validate_commercial_terms(self):
        if self.min_fill_mt > self.quantity_mt:
            raise ValueError("Minimum fill must not exceed offered quantity")
        if self.delivery_start > self.delivery_end:
            raise ValueError("Delivery end must not precede delivery start")
        if self.fuel_terms.certificate_valid_until < self.delivery_end:
            raise ValueError(
                "Declared operator certificate must remain valid through delivery"
            )
        evidence = self.fuel_terms.sustainability_evidence
        if evidence and evidence.due != self.evidence_due:
            raise ValueError(
                "Sustainability evidence due milestone must match the listing terms"
            )
        return self


class SupplierOfferUpdate(SupplierOfferCreate):
    expected_revision: int = Field(ge=1)


class SupplierOfferWithdraw(BaseModel):
    model_config = ConfigDict(extra="forbid")
    expected_revision: int = Field(ge=1)


class FameQualitySummary(BaseModel):
    status: Literal["PENDING", "AVAILABLE"]
    results: list[FameQualityResult]


class FameSustainabilitySummary(BaseModel):
    status: Literal["PENDING", "AVAILABLE"]
    document_type: Literal["POS", "SD", "POC"]
    due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"]


class FameFuelSummary(BaseModel):
    """No supplier identity, batch references, certificate IDs or document links."""

    schema_version: Literal[1] = 1
    neat_fame: Literal[True] = True
    nomination_status: Literal["PENDING", "IDENTIFIED"]
    uco_mass_pct: Literal[100]
    standard: FuelStandard
    standard_edition: str
    astm_grade: ASTMGrade | None = None
    en_climate_class: str | None = None
    production_origin: str | None = None
    feedstock_origin: str | None = None
    shipping_location: str | None = None
    cfpp_c: Decimal | None = None
    cloud_point_c: Decimal | None = None
    ci_gco2e_mj: Decimal | None = None
    ci_methodology: str | None = None
    ci_boundary: str | None = None
    ci_basis: Literal["ACTUAL", "DEFAULT"] | None = None
    lhv_mj_kg: Decimal | None = None
    sustainability_scheme: SustainabilityScheme
    certificate_valid_until: date
    evidence_status: Literal["DECLARED", "PENDING", "AVAILABLE"]
    quality_evidence: FameQualitySummary | None = None
    sustainability_evidence: FameSustainabilitySummary | None = None


class SupplierOfferPublicTerms(SupplierOfferCommercialTerms):
    fuel_terms: FameFuelSummary


class SupplierOfferResponse(SupplierOfferCommercialTerms):
    id: UUID
    product_id: UUID
    delivery_point_id: UUID
    product_name: str = "UCOME B100"
    market_product: Literal["UCOME_B100"] = "UCOME_B100"
    delivery_point_name: str = "Singapore"
    quantity_mt: Decimal
    min_fill_mt: Decimal
    price_per_mt_usd: Decimal
    availability_window: str
    fuel_terms: FameListingFuelTerms | FameFuelSummary
    status: Literal["OPEN", "WITHDRAWN", "EXPIRED"]
    revision: int
    expires_at: datetime
    created_at: datetime
    updated_at: datetime
    listing_kind: Literal["SUPPLIER_OFFER"] = "SUPPLIER_OFFER"
    execution_enabled: Literal[False] = False
    can_edit: bool = False
    can_withdraw: bool = False
    can_request_quote: bool = False


class SupplierOfferList(BaseModel):
    items: list[SupplierOfferResponse]
    total: int
    skip: int
    limit: int


class SupplierOfferSnapshotValues(BaseModel):
    offer_id: UUID
    revision: int
    product_id: UUID
    delivery_point_id: UUID
    quantity_mt: Decimal
    min_fill_mt: Decimal
    price_per_mt_usd: Decimal
    availability_window: str
    expires_at: datetime


class SupplierOfferSnapshot(SupplierOfferSnapshotValues):
    supplier_org_id: UUID
    listing_terms: SupplierOfferTerms


class SupplierOfferPublicSnapshot(SupplierOfferSnapshotValues):
    listing_terms: SupplierOfferPublicTerms

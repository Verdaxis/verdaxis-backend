"""Declared UCOME B100 contract terms; these fields do not verify compliance."""
from datetime import date
from decimal import Decimal
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator


ShortText = Annotated[str, Field(min_length=1, max_length=200)]
TermsText = Annotated[str, Field(min_length=1, max_length=1000)]
FuelStandard = Literal["EN_14214", "ASTM_D6751"]
SustainabilityScheme = Literal["ISCC_EU", "REDCERT_EU"]
Temperature = Annotated[Decimal, Field(ge=-80, le=50, allow_inf_nan=False)]
CarbonIntensity = Annotated[Decimal, Field(ge=0, le=200, allow_inf_nan=False)]


class DeclaredTerms(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)

    schema_version: Literal[1] = 1


class FameContractTerms(DeclaredTerms):
    neat_fame: Literal[True]
    standard: FuelStandard
    standard_edition: ShortText
    max_cfpp_c: Temperature | None = None
    max_ci_gco2e_mj: CarbonIntensity | None = None
    delivery_basis: Literal["EX_TANK", "FOB", "FCA", "CIF"]
    named_location: ShortText
    delivery_start: date
    delivery_end: date
    quantity_tolerance_pct: Decimal = Field(ge=0, le=10, allow_inf_nan=False)
    min_fill_mt: Decimal = Field(gt=0, le=100000, decimal_places=2, allow_inf_nan=False)
    payment_terms: TermsText
    inspection_terms: TermsText
    title_risk_terms: TermsText
    claims_terms: TermsText
    sustainability_scheme: SustainabilityScheme
    evidence_due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"]

    @model_validator(mode="after")
    def validate_delivery_window(self):
        if self.delivery_end < self.delivery_start:
            raise ValueError("delivery_end must be on or after delivery_start")
        return self


class FameOfferTerms(DeclaredTerms):
    matches_contract_terms: Literal[True]
    batch_reference: ShortText
    producing_site: ShortText
    production_origin: ShortText
    feedstock_origin: ShortText
    shipping_location: ShortText
    uco_mass_pct: Literal[100]
    standard: FuelStandard
    standard_edition: ShortText
    cfpp_c: Temperature | None = None
    ci_gco2e_mj: CarbonIntensity | None = None
    ci_methodology: ShortText | None = None
    sustainability_scheme: SustainabilityScheme
    certificate_reference: ShortText
    certificate_holder: ShortText
    certificate_valid_until: date
    evidence_status: Literal["DECLARED", "PENDING", "AVAILABLE"]
    document_references: list[ShortText] = Field(default_factory=list, max_length=20)
    available_quantity_mt: Decimal = Field(gt=0, le=100000, decimal_places=2, allow_inf_nan=False)
    lhv_mj_kg: Decimal | None = Field(None, gt=0, le=50, allow_inf_nan=False)

    @model_validator(mode="after")
    def validate_evidence_declarations(self):
        if self.ci_gco2e_mj is not None and not self.ci_methodology:
            raise ValueError("ci_methodology is required with declared carbon intensity")
        if self.evidence_status == "AVAILABLE" and not self.document_references:
            raise ValueError("document_references are required when evidence is AVAILABLE")
        return self

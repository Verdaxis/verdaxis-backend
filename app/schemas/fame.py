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
ASTMGrade = Literal[
    "1-B S15", "1-B S15 LM", "1-B S500", "2-B S15", "2-B S15 LM", "2-B S500"
]
QualityProperty = Literal[
    "ESTER_CONTENT_MASS_PCT",
    "DENSITY_15C_KG_M3",
    "KINEMATIC_VISCOSITY_40C_MM2_S",
    "WATER_MG_KG",
    "WATER_AND_SEDIMENT_VOL_PCT",
    "ACID_VALUE_MG_KOH_G",
    "OXIDATION_STABILITY_H",
    "SULFUR_MG_KG",
    "FLASH_POINT_C",
    "FREE_GLYCEROL_MASS_PCT",
    "TOTAL_GLYCEROL_MASS_PCT",
    "MONOGLYCERIDES_MASS_PCT",
    "PHOSPHORUS_MG_KG",
    "SODIUM_POTASSIUM_MG_KG",
    "CALCIUM_MAGNESIUM_MG_KG",
    "METHANOL_MASS_PCT",
    "TOTAL_CONTAMINATION_MG_KG",
]


class DeclaredTerms(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)

    schema_version: Literal[1] = 1


class FameQualityResult(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)

    property: QualityProperty
    value: Decimal = Field(allow_inf_nan=False)
    method: ShortText | None = None

    @model_validator(mode="after")
    def validate_physical_range(self):
        # Units are fixed by the property name. These are storage bounds,
        # never a claim that a result passes EN 14214 or ASTM D6751.
        if self.property == "FLASH_POINT_C":
            lower, upper = Decimal("-100"), Decimal("1000")
        elif self.property.endswith("_PCT"):
            lower, upper = Decimal("0"), Decimal("100")
        elif self.property.endswith("_MG_KG"):
            lower, upper = Decimal("0"), Decimal("1000000")
        else:
            lower, upper = Decimal("0"), Decimal("10000")
        if not lower <= self.value <= upper:
            raise ValueError(
                f"{self.property} must be between {lower} and {upper} in its stated units"
            )
        return self


class FameQualityEvidence(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)

    status: Literal["PENDING", "AVAILABLE"]
    reference: ShortText | None = None
    batch_reference: ShortText | None = None
    laboratory: ShortText | None = None
    sampled_on: date | None = None
    tested_on: date | None = None
    results: list[FameQualityResult] = Field(default_factory=list, max_length=30)

    @model_validator(mode="after")
    def validate_traceability(self):
        if self.status == "AVAILABLE" and not all(
            (self.reference, self.batch_reference, self.laboratory, self.tested_on)
        ):
            raise ValueError(
                "Available quality evidence requires reference, batch, laboratory and test date"
            )
        if self.sampled_on and self.tested_on and self.sampled_on > self.tested_on:
            raise ValueError("Sampling date must not follow test date")
        if len({result.property for result in self.results}) != len(self.results):
            raise ValueError("Each quality property may occur once per evidence record")
        return self


class FameSustainabilityEvidence(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)

    status: Literal["PENDING", "AVAILABLE"]
    document_type: Literal["POS", "SD", "POC"]
    reference: ShortText | None = None
    issuer: ShortText | None = None
    quantity_mt: Decimal | None = Field(
        None, gt=0, le=100000, decimal_places=2, allow_inf_nan=False
    )
    supply_date: date | None = None
    due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"]

    @model_validator(mode="after")
    def validate_document_reference(self):
        if self.status == "AVAILABLE" and not self.reference:
            raise ValueError(
                "Available sustainability evidence requires a document reference"
            )
        return self


class FameContractTerms(DeclaredTerms):
    neat_fame: Literal[True]
    standard: FuelStandard
    standard_edition: ShortText
    astm_grade: ASTMGrade | None = None
    en_climate_class: ShortText | None = None
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


class FameFuelDeclaration(DeclaredTerms):
    neat_fame: Literal[True] = True
    batch_reference: ShortText
    producing_site: ShortText
    production_origin: ShortText
    feedstock_origin: ShortText
    shipping_location: ShortText
    uco_mass_pct: Literal[100]
    standard: FuelStandard
    standard_edition: ShortText
    astm_grade: ASTMGrade | None = None
    en_climate_class: ShortText | None = None
    cfpp_c: Temperature | None = None
    cloud_point_c: Decimal | None = Field(None, ge=-80, le=80, allow_inf_nan=False)
    ci_gco2e_mj: CarbonIntensity | None = None
    ci_methodology: ShortText | None = None
    ci_boundary: ShortText | None = None
    ci_basis: Literal["ACTUAL", "DEFAULT"] | None = None
    sustainability_scheme: SustainabilityScheme
    certificate_reference: ShortText
    certificate_holder: ShortText
    certificate_valid_until: date
    certificate_scope: ShortText | None = None
    evidence_status: Literal["DECLARED", "PENDING", "AVAILABLE"]
    document_references: list[ShortText] = Field(default_factory=list, max_length=20)
    lhv_mj_kg: Decimal | None = Field(None, gt=0, le=50, allow_inf_nan=False)
    quality_evidence: FameQualityEvidence | None = None
    sustainability_evidence: FameSustainabilityEvidence | None = None

    @model_validator(mode="after")
    def validate_evidence_declarations(self):
        if self.ci_gco2e_mj is not None and not self.ci_methodology:
            raise ValueError(
                "ci_methodology is required with declared carbon intensity"
            )
        if self.evidence_status == "AVAILABLE" and not self.document_references:
            raise ValueError(
                "document_references are required when evidence is AVAILABLE"
            )
        quality = self.quality_evidence
        if (
            quality
            and quality.status == "AVAILABLE"
            and self.batch_reference
            and quality.batch_reference != self.batch_reference
        ):
            raise ValueError(
                "Quality evidence batch must match the nominated fuel batch"
            )
        return self


class FameOfferTerms(FameFuelDeclaration):
    matches_contract_terms: Literal[True]
    available_quantity_mt: Decimal = Field(
        gt=0, le=100000, decimal_places=2, allow_inf_nan=False
    )


class FameListingFuelTerms(FameFuelDeclaration):
    neat_fame: Literal[True]
    nomination_status: Literal["PENDING", "IDENTIFIED"] = "PENDING"
    batch_reference: ShortText | None = None
    producing_site: ShortText | None = None
    production_origin: ShortText | None = None
    feedstock_origin: ShortText | None = None
    shipping_location: ShortText | None = None

    @model_validator(mode="after")
    def validate_listing_declaration(self):
        if self.standard == "ASTM_D6751" and self.astm_grade is None:
            raise ValueError("ASTM D6751 listings require the declared grade")
        if self.standard != "ASTM_D6751" and self.astm_grade is not None:
            raise ValueError("ASTM grade requires ASTM D6751 as the standard")
        if self.nomination_status == "IDENTIFIED" and not all(
            (
                self.batch_reference,
                self.producing_site,
                self.production_origin,
                self.feedstock_origin,
                self.shipping_location,
            )
        ):
            raise ValueError(
                "Identified supply requires batch, producing site and origin/location details"
            )
        if self.ci_gco2e_mj is not None and not all((self.ci_boundary, self.ci_basis)):
            raise ValueError(
                "Declared listing carbon intensity requires boundary and ACTUAL/DEFAULT basis"
            )
        return self

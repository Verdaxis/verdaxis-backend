"""B100 requirements and declarations within the shared order book."""

from decimal import Decimal
from typing import Annotated, Literal

from pydantic import Field, model_validator

from app.schemas.fame import (
    ASTMGrade,
    CarbonIntensity,
    DeclaredTerms,
    FameListingFuelTerms,
    FuelStandard,
    ShortText,
    SustainabilityScheme,
    Temperature,
)
from app.schemas.supplier_offer import FameFuelSummary


class FameBidTerms(DeclaredTerms):
    side: Literal["BID"]
    neat_fame: Literal[True]
    standard: FuelStandard
    standard_edition: ShortText
    astm_grade: ASTMGrade | None = None
    en_climate_class: ShortText | None = None
    max_cfpp_c: Temperature | None = None
    max_cloud_point_c: Decimal | None = Field(None, ge=-80, le=80, allow_inf_nan=False)
    max_ci_gco2e_mj: CarbonIntensity | None = None
    ci_methodology: ShortText | None = None
    ci_boundary: ShortText | None = None
    ci_basis: Literal["ACTUAL", "DEFAULT"] | None = None
    sustainability_scheme: SustainabilityScheme
    require_quality_evidence: bool = False
    require_sustainability_evidence: bool = False
    evidence_due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"] = "BEFORE_LOADING"

    @model_validator(mode="after")
    def validate_requirements(self):
        if self.standard == "ASTM_D6751" and self.astm_grade is None:
            raise ValueError("ASTM D6751 orders require a grade")
        if self.standard != "ASTM_D6751" and self.astm_grade is not None:
            raise ValueError("ASTM grade requires ASTM D6751 as the standard")
        if self.max_ci_gco2e_mj is not None and not all(
            (self.ci_methodology, self.ci_boundary, self.ci_basis)
        ):
            raise ValueError(
                "A CI limit requires methodology, boundary and ACTUAL/DEFAULT basis"
            )
        return self


class FameAskTerms(FameListingFuelTerms):
    side: Literal["ASK"]
    standard_edition: str = Field(min_length=1, max_length=100)
    ci_gco2e_mj: Decimal | None = Field(
        None, ge=0, le=200, decimal_places=2, allow_inf_nan=False
    )
    ci_methodology: str | None = Field(None, min_length=1, max_length=120)
    lhv_mj_kg: Decimal | None = Field(
        None, gt=0, le=50, decimal_places=2, allow_inf_nan=False
    )
    evidence_due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"]

    @model_validator(mode="after")
    def validate_evidence_commitment(self):
        if (
            self.sustainability_evidence
            and self.sustainability_evidence.due != self.evidence_due
        ):
            raise ValueError(
                "Sustainability document milestone must match the order commitment"
            )
        return self


class FamePublicAskTerms(FameFuelSummary):
    side: Literal["ASK"]
    evidence_due: Literal["BEFORE_LOADING", "BEFORE_DELIVERY"]


FameOrderTerms = Annotated[FameBidTerms | FameAskTerms, Field(discriminator="side")]
FamePublicOrderTerms = Annotated[
    FameBidTerms | FamePublicAskTerms, Field(discriminator="side")
]


class FameTradeSnapshot(DeclaredTerms):
    bid: FameBidTerms
    ask: FameAskTerms


class FamePublicTradeSnapshot(DeclaredTerms):
    bid: FameBidTerms
    ask: FamePublicAskTerms

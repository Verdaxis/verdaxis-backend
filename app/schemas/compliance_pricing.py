"""Schemas for the compliance-adjusted pricing overlay endpoint (H1.2)."""
from decimal import Decimal
from typing import Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field


class PricingOverlayRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")

    order_ids: list[UUID] = Field(
        ...,
        max_length=100,
        description="Marketplace order ids to price (max 100 per request)",
    )
    year: int = Field(
        2026,
        ge=2025,
        description=(
            "Compliance year, annotation-only: it selects the reported "
            "year_target but the marginal displacement math has no year term"
        ),
    )


class ListingOverlay(BaseModel):
    """Declared lifecycle comparison; legacy financial fields stay zero.

    Zero here means no benefit is priced, not a verified zero entitlement.
    """

    financial_benefit_status: Literal["UNPRICED"] = "UNPRICED"
    penalty_avoided_eur_per_mt: Decimal
    penalty_avoided_usd_per_mt: Decimal
    tco2e_avoided_per_mt: Decimal
    ci_gco2_mj: Decimal
    ci_basis: Literal["LISTING", "PRODUCT_DEFAULT"]
    lcv_mj_kg: Decimal
    lcv_basis: Literal["LISTING", "PRODUCT_DEFAULT"]


class OverlayAssumptions(BaseModel):
    """Every assumption behind the numbers, so the UI can label them."""

    eur_usd_rate: Decimal
    vlsfo_baseline_gco2_mj: Decimal
    ghgie_actual_gco2_mj: Decimal
    fleet_intensity_basis: Literal["ORG_FLEET", "DEFAULT_VLSFO"]
    fleet_vessel_count: int
    penalty_eur_per_tonne: Decimal
    year: int
    year_target: Decimal
    excluded_factors: list[str]


class PricingOverlayResponse(BaseModel):
    overlays: dict[UUID, ListingOverlay | None]
    assumptions: OverlayAssumptions

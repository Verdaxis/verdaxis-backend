"""
Compliance scoring API endpoints.
Provides vessel-level compliance assessments.
"""
from decimal import Decimal
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.routers.orderbook import _apply_public_marketplace_scope
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import User, UserRole
from app.models.port import Vessel
from app.schemas.compliance_pricing import (
    ListingOverlay,
    PricingOverlayRequest,
    PricingOverlayResponse,
)
from app.services.compliance_pricing import compute_listing_overlay, overlay_assumptions
from app.services.compliance_scoring import (
    calculate_compliance_score,
    ComplianceScore,
    FUEL_GHG_INTENSITIES,
)

router = APIRouter(prefix="/compliance", tags=["compliance"])


# --- Pydantic response models ---

class FuelEUResponse(BaseModel):
    ghg_intensity_gco2_mj: Decimal
    target_intensity_gco2_mj: Decimal
    reduction_pct: Decimal
    compliance_balance_gco2: Decimal
    estimated_penalty_eur: Decimal
    score: int

class EUETSResponse(BaseModel):
    total_co2_tonnes: Decimal
    ets_price_per_tonne_eur: Decimal
    phase_in_pct: Decimal
    estimated_cost_eur: Decimal
    score: int

class CIIResponse(BaseModel):
    rating: str
    score: int

class ComplianceScoreResponse(BaseModel):
    vessel_id: str
    vessel_name: str
    overall_score: int
    status: str
    traffic_light: str
    fueleu: FuelEUResponse
    eu_ets: EUETSResponse
    cii: CIIResponse
    recommendations: list[str]

class FleetComplianceSummary(BaseModel):
    total_vessels: int
    green_count: int
    amber_count: int
    red_count: int
    average_score: float
    vessels: list[ComplianceScoreResponse]

class FuelMixInput(BaseModel):
    fuel_mix: dict[str, Decimal] = Field(
        default={"VLSFO": Decimal("1.0")},
        description="Fuel mix fractions (must sum to 1.0)"
    )

class ScenarioInput(BaseModel):
    vessel_id: str
    fuel_mix: dict[str, Decimal]
    year: int = 2026


# --- Endpoints ---

@router.get("/vessels/{vessel_id}/score", response_model=ComplianceScoreResponse)
async def get_vessel_compliance(
    vessel_id: str,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """Get compliance score for a specific vessel."""
    stmt = select(Vessel).where(Vessel.id == vessel_id)
    result = await db.execute(stmt)
    vessel = result.scalar_one_or_none()

    if not vessel:
        raise HTTPException(status_code=404, detail="Vessel not found")

    # Check org access (users can only see their org's vessels, admins see all)
    if current_user.role != UserRole.ADMIN and vessel.organization_id != current_user.organization_id:
        raise HTTPException(status_code=403, detail="Not authorized to view this vessel")

    score = calculate_compliance_score(
        vessel_id=str(vessel.id),
        vessel_name=vessel.name,
        cii_rating=vessel.cii_rating,
        vessel_type=vessel.vessel_type or "default",
    )

    return _score_to_response(score)


@router.get("/fleet", response_model=FleetComplianceSummary)
async def get_fleet_compliance(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """Get compliance scores for all vessels in user's organization."""
    stmt = select(Vessel)
    if current_user.role != UserRole.ADMIN:
        stmt = stmt.where(Vessel.organization_id == current_user.organization_id)

    result = await db.execute(stmt)
    vessels = result.scalars().all()

    scores = []
    for vessel in vessels:
        score = calculate_compliance_score(
            vessel_id=str(vessel.id),
            vessel_name=vessel.name,
            cii_rating=vessel.cii_rating,
            vessel_type=vessel.vessel_type or "default",
        )
        scores.append(_score_to_response(score))

    green = sum(1 for s in scores if s.traffic_light == "GREEN")
    amber = sum(1 for s in scores if s.traffic_light == "AMBER")
    red = sum(1 for s in scores if s.traffic_light == "RED")
    avg = sum(s.overall_score for s in scores) / len(scores) if scores else 0

    return FleetComplianceSummary(
        total_vessels=len(scores),
        green_count=green,
        amber_count=amber,
        red_count=red,
        average_score=round(avg, 1),
        vessels=scores,
    )


@router.post("/scenario", response_model=ComplianceScoreResponse)
async def run_compliance_scenario(
    scenario: ScenarioInput,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Run a what-if scenario: 'What if I use this fuel mix?'
    Returns projected compliance score without saving anything.
    """
    vessel = None
    try:
        from uuid import UUID as _UUID
        _UUID(scenario.vessel_id)  # Validate it's a UUID
        stmt = select(Vessel).where(Vessel.id == scenario.vessel_id)
        result = await db.execute(stmt)
        vessel = result.scalar_one_or_none()
    except ValueError:
        pass  # Non-UUID vessel_id or not found — use defaults

    # Authorization check: don't leak cross-org vessel data
    if vessel:
        from app.models.user import UserRole
        if current_user.role != UserRole.ADMIN and vessel.organization_id != current_user.organization_id:
            vessel = None  # Fall back to defaults

    vessel_name = vessel.name if vessel else f"Vessel {scenario.vessel_id}"
    cii_rating = vessel.cii_rating if vessel else None
    vessel_type = vessel.vessel_type if vessel else "default"

    score = calculate_compliance_score(
        vessel_id=scenario.vessel_id,
        vessel_name=vessel_name,
        fuel_mix=scenario.fuel_mix,
        cii_rating=cii_rating,
        vessel_type=vessel_type,
        year=scenario.year,
    )

    return _score_to_response(score)


@router.post("/pricing-overlay", response_model=PricingOverlayResponse)
async def get_pricing_overlay(
    payload: PricingOverlayRequest,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
) -> PricingOverlayResponse:
    """Batch FuelEU pricing overlay for marketplace ASK listings.

    POST is used for a read: up to 100 order ids do not fit reliably in a
    query string. Nothing is persisted.

    Prices what the green premium buys the buyer under FuelEU Maritime when
    the listed fuel displaces VLSFO: marginal penalty avoided per MT (EUR and
    USD) and tCO2e avoided per MT. Distinct from GET /orderbook/with-ci,
    whose CIAdjustedPrice values avoided CO2 at the EU ETS carbon price -- a
    much smaller number than FuelEU penalty avoidance. Indicative estimate:
    the RFNBO reward multiplier, consecutive-deficit escalation and the 50%
    extra-EU voyage scope are excluded and named in
    assumptions.excluded_factors.

    Only ASK rows visible in the public marketplace book are priced. Bid
    ids, unknown ids, cancelled/hidden/off-spec rows and rows whose CI or
    LCV cannot be resolved all return null indistinguishably (no existence
    oracle).
    """
    filters: list[object] = [
        OrderBookOrder.id.in_(payload.order_ids),
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        OrderBookOrder.expires_at.is_(None) | (OrderBookOrder.expires_at > func.now()),
        OrderBookOrder.side == OrderSide.ASK,
    ]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(filters, joins)

    query = select(OrderBookOrder)
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters)
    orders = (await db.execute(query)).unique().scalars().all()

    overlays: dict[UUID, ListingOverlay | None] = {
        order_id: None for order_id in payload.order_ids
    }
    for order in orders:
        overlays[order.id] = compute_listing_overlay(
            market_product=order.market_product,
            listing_ci_gco2_mj=order.carbon_intensity_gco2_mj,
            listing_lcv_mj_kg=order.energy_density_mj_kg,
        )

    # Org awareness mirrors /fleet reads: admins see the whole fleet, other
    # users their organization's vessels. Count only -- the prototype always
    # computes with the default GHGIE_actual (see overlay_assumptions).
    vessel_count_stmt = select(func.count()).select_from(Vessel)
    if current_user.role != UserRole.ADMIN:
        if current_user.organization_id is None:
            vessel_count_stmt = None
        else:
            vessel_count_stmt = vessel_count_stmt.where(
                Vessel.organization_id == current_user.organization_id
            )
    fleet_vessel_count = (
        (await db.execute(vessel_count_stmt)).scalar_one() if vessel_count_stmt is not None else 0
    )

    return PricingOverlayResponse(
        overlays=overlays,
        assumptions=overlay_assumptions(payload.year, fleet_vessel_count),
    )


@router.get("/fuels", response_model=dict)
async def list_fuel_intensities():
    """List all known fuel types and their GHG intensities."""
    return {
        "fuels": {k: str(v) for k, v in FUEL_GHG_INTENSITIES.items()},
        "unit": "gCO2eq/MJ",
        "source": "FuelEU Maritime Regulation (EU) 2023/1805",
    }


def _score_to_response(score: ComplianceScore) -> ComplianceScoreResponse:
    return ComplianceScoreResponse(
        vessel_id=score.vessel_id,
        vessel_name=score.vessel_name,
        overall_score=score.overall_score,
        status=score.status.value,
        traffic_light=score.traffic_light.value,
        fueleu=FuelEUResponse(
            ghg_intensity_gco2_mj=score.fueleu.ghg_intensity_gco2_mj,
            target_intensity_gco2_mj=score.fueleu.target_intensity_gco2_mj,
            reduction_pct=score.fueleu.reduction_pct,
            compliance_balance_gco2=score.fueleu.compliance_balance_gco2,
            estimated_penalty_eur=score.fueleu.estimated_penalty_eur,
            score=score.fueleu.score,
        ),
        eu_ets=EUETSResponse(
            total_co2_tonnes=score.eu_ets.total_co2_tonnes,
            ets_price_per_tonne_eur=score.eu_ets.ets_price_per_tonne_eur,
            phase_in_pct=score.eu_ets.phase_in_pct,
            estimated_cost_eur=score.eu_ets.estimated_cost_eur,
            score=score.eu_ets.score,
        ),
        cii=CIIResponse(rating=score.cii.rating, score=score.cii.score),
        recommendations=score.recommendations,
    )

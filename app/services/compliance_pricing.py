"""Declared lifecycle comparison for marketplace ASK listings.

Financial benefits are unpriced: the API does not establish eligible annual
consumption, an actual compliance deficit, or contractual pooling value.
Legacy penalty-avoided fields are zero, not a prediction of earned savings.
"""
from decimal import ROUND_HALF_UP, Decimal

from app.config import settings
from app.schemas.compliance_pricing import ListingOverlay, OverlayAssumptions
from app.services.fueleu import FUELEU_REFERENCE_GHG, fueleu_year_target

# Reference for the lifecycle comparison, not a verified displaced fuel.
VLSFO_BASELINE_GCO2_MJ = FUELEU_REFERENCE_GHG

# Legacy scenario metadata, not measured fleet intensity.
DEFAULT_GHGIE_ACTUAL_GCO2_MJ = Decimal("91.16")

# FuelEU penalty: EUR 2,400 per tonne of VLSFO-energy-equivalent deficit
# (Regulation (EU) 2023/1805), spread over 41,000 MJ/tonne of VLSFO.
PENALTY_EUR_PER_TONNE = Decimal("2400")

# ASSUMED conversion rate; override via settings.COMPLIANCE_EUR_USD_RATE.
EUR_USD_RATE = Decimal("1.08")

GRAMS_PER_TONNE = Decimal("1000000")
KG_PER_MT = Decimal("1000")

# Default lower calorific value per market product (MJ/kg).
PRODUCT_DEFAULT_LCV: dict[str, Decimal] = {
    "BIO_METHANOL": Decimal("19.9"),
    "E_METHANOL": Decimal("19.9"),
    "BIO_ETHANOL": Decimal("26.8"),
    "SYNTHETIC_ETHANOL": Decimal("26.8"),
}

# Additional regulatory factors not established by this lifecycle comparison.
EXCLUDED_FACTORS: tuple[str, ...] = (
    "RFNBO_MULTIPLIER",
    "DEFICIT_ESCALATION",
    "EXTRA_EU_VOYAGE_SCOPE",
)

_TCO2E = Decimal("0.001")


def _effective_eur_usd_rate() -> Decimal:
    override = settings.COMPLIANCE_EUR_USD_RATE
    return override if override is not None else EUR_USD_RATE


def compute_listing_overlay(
    *,
    market_product: str | None,
    listing_ci_gco2_mj: Decimal | None,
    listing_lcv_mj_kg: Decimal | None,
    ghgie_actual_gco2_mj: Decimal = DEFAULT_GHGIE_ACTUAL_GCO2_MJ,
    eur_usd_rate: Decimal | None = None,
) -> ListingOverlay | None:
    """Compare declared lifecycle CI on an equal-energy basis.

    Unknown CI stays unknown. Product labels do not establish consignment
    emissions or regulatory eligibility. Known family LCV values can still
    support an explicitly labelled physical comparison. Legacy fleet/FX
    arguments remain accepted for callers; they cannot create cash benefits.
    """
    if listing_ci_gco2_mj is None or not listing_ci_gco2_mj.is_finite():
        return None
    ci, ci_basis = listing_ci_gco2_mj, "LISTING"

    if listing_lcv_mj_kg is not None:
        lcv, lcv_basis = listing_lcv_mj_kg, "LISTING"
    elif market_product in PRODUCT_DEFAULT_LCV:
        lcv, lcv_basis = PRODUCT_DEFAULT_LCV[market_product], "PRODUCT_DEFAULT"
    else:
        return None

    if not lcv.is_finite() or lcv <= 0:
        return None

    # Equal-energy lifecycle difference per MT of fuel, in grams:
    # (CI_displaced - CI_g) [g/MJ] * LCV_g [MJ/kg] * 1000 [kg/MT].
    displacement_g_per_mt = max(
        Decimal("0"), (VLSFO_BASELINE_GCO2_MJ - ci) * lcv * KG_PER_MT
    )

    # Without annual eligible consumption, deficit and owned pooling terms,
    # a marginal penalty proxy cannot be booked as an avoided cash expense.
    return ListingOverlay(
        penalty_avoided_eur_per_mt=Decimal("0.00"),
        penalty_avoided_usd_per_mt=Decimal("0.00"),
        tco2e_avoided_per_mt=(displacement_g_per_mt / GRAMS_PER_TONNE).quantize(
            _TCO2E, rounding=ROUND_HALF_UP
        ),
        ci_gco2_mj=ci,
        ci_basis=ci_basis,
        lcv_mj_kg=lcv,
        lcv_basis=lcv_basis,
    )


def overlay_assumptions(year: int, fleet_vessel_count: int) -> OverlayAssumptions:
    """Assumptions behind every overlay in a response.

    Vessel count is context only. Having registered vessels does not provide
    an actual annual fleet intensity; the basis stays DEFAULT_VLSFO.
    """
    return OverlayAssumptions(
        eur_usd_rate=_effective_eur_usd_rate(),
        vlsfo_baseline_gco2_mj=VLSFO_BASELINE_GCO2_MJ,
        ghgie_actual_gco2_mj=DEFAULT_GHGIE_ACTUAL_GCO2_MJ,
        fleet_intensity_basis="DEFAULT_VLSFO",
        fleet_vessel_count=fleet_vessel_count,
        penalty_eur_per_tonne=PENALTY_EUR_PER_TONNE,
        year=year,
        year_target=fueleu_year_target(year),
        excluded_factors=list(EXCLUDED_FACTORS),
    )

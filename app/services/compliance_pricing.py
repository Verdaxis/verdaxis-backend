"""FuelEU Maritime compliance-adjusted pricing overlay (H1.2 prototype).

Computes, per marketplace ASK listing, what the green premium buys a buyer
under FuelEU: tCO2e avoided per MT and marginal penalty avoided per MT when
the listed fuel displaces VLSFO.

The two roles of GHG intensity are kept separate on purpose:

- ``VLSFO_BASELINE_GCO2_MJ`` is the intensity of the *displaced* fuel; it is
  fixed at 91.16 gCO2e/MJ.
- ``ghgie_actual_gco2_mj`` is the fleet's actual intensity, which sets the
  Annex IV marginal penalty rate ``2400 / (GHGIE_actual * 41000)`` EUR per
  gram of compliance balance. The prototype always uses 91.16, but a
  lower-intensity fleet must not silently shrink the displacement term, so
  the symbol stays distinct for the H1.1 fleet engine to drop in.

Distinct from ``ci_pricing.calculate_ci_adjusted_price``, which values
avoided CO2 at the EU ETS carbon price (a much smaller number); and from
``compliance_scoring``, whose simplified vessel penalty math is a known
follow-up. Neither module is touched here.

Pure module: no I/O, no DB. Money quantized to 0.01, tCO2e to 0.001.
"""
from decimal import ROUND_HALF_UP, Decimal

from app.config import settings
from app.schemas.compliance_pricing import ListingOverlay, OverlayAssumptions

# Intensity of the displaced fuel (VLSFO, gCO2e/MJ well-to-wake). Same
# baseline the ASK metadata is judged against; ci_pricing's 91.0 is the
# outlier, noted and untouched.
VLSFO_BASELINE_GCO2_MJ = Decimal("91.16")

# Prototype fleet actual GHG intensity (gCO2e/MJ) for the marginal rate
# denominator. H1.1 derives this per organization; until then the default
# fleet is the real path, not an error.
DEFAULT_GHGIE_ACTUAL_GCO2_MJ = Decimal("91.16")

# FuelEU penalty: EUR 2,400 per tonne of VLSFO-energy-equivalent deficit
# (Regulation (EU) 2023/1805), spread over 41,000 MJ/tonne of VLSFO.
PENALTY_EUR_PER_TONNE = Decimal("2400")
VLSFO_MJ_PER_TONNE = Decimal("41000")

# ASSUMED conversion rate; override via settings.COMPLIANCE_EUR_USD_RATE.
EUR_USD_RATE = Decimal("1.08")

GRAMS_PER_TONNE = Decimal("1000000")
KG_PER_MT = Decimal("1000")

# Default CI per market product (gCO2e/MJ well-to-wake) for listings without
# a declared CI. Methanol values match the FUEL_GHG_INTENSITIES entries
# ("Methanol", "E-Methanol"); BIO_ETHANOL is a Biofuel-class proxy and
# SYNTHETIC_ETHANOL an e-fuel-class proxy. Deliberately NOT resolved through
# compliance_scoring.FUEL_GHG_INTENSITIES: Ethanol is absent there, and its
# ``.get(fuel, 91.16)`` fallback silently zeroes the advantage of any fuel
# it does not know.
PRODUCT_DEFAULT_CI: dict[str, Decimal] = {
    "BIO_METHANOL": Decimal("31"),
    "E_METHANOL": Decimal("8"),
    "BIO_ETHANOL": Decimal("35"),
    "SYNTHETIC_ETHANOL": Decimal("10"),
}

# Default lower calorific value per market product (MJ/kg).
PRODUCT_DEFAULT_LCV: dict[str, Decimal] = {
    "BIO_METHANOL": Decimal("19.9"),
    "E_METHANOL": Decimal("19.9"),
    "BIO_ETHANOL": Decimal("26.8"),
    "SYNTHETIC_ETHANOL": Decimal("26.8"),
}

# Named exclusions from the marginal math, surfaced in every response: the
# RFNBO x2 reward multiplier (materially understates E_METHANOL /
# SYNTHETIC_ETHANOL value through 2033), consecutive-deficit escalation
# (x(1+(n-1)/10)), and the 50% extra-EU voyage scope.
EXCLUDED_FACTORS: tuple[str, ...] = (
    "RFNBO_MULTIPLIER",
    "DEFICIT_ESCALATION",
    "EXTRA_EU_VOYAGE_SCOPE",
)

_MONEY = Decimal("0.01")
_TCO2E = Decimal("0.001")


def fueleu_year_target(year: int) -> Decimal:
    """FuelEU GHG intensity target for ``year``, annotation-only.

    Explicit step function (2025-2029 -> 89.34, 2030-2034 -> 80.04,
    2035-2049 -> 65.08, 2050+ -> 9.12). ``FUELEU_TARGETS.get(year, default)``
    would mis-report every year without a literal dict key (2031+).
    """
    if year >= 2050:
        return Decimal("9.12")
    if year >= 2035:
        return Decimal("65.08")
    if year >= 2030:
        return Decimal("80.04")
    return Decimal("89.34")


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
    """Price the FuelEU advantage of one ASK listing, or None if unpriceable.

    Listing-declared CI/LCV win over the product defaults; a row whose CI or
    LCV resolves neither way returns None (never raises). The displacement
    term is floored at zero: a fuel at or above the VLSFO baseline avoids
    nothing.
    """
    if listing_ci_gco2_mj is not None:
        ci, ci_basis = listing_ci_gco2_mj, "LISTING"
    elif market_product in PRODUCT_DEFAULT_CI:
        ci, ci_basis = PRODUCT_DEFAULT_CI[market_product], "PRODUCT_DEFAULT"
    else:
        return None

    if listing_lcv_mj_kg is not None:
        lcv, lcv_basis = listing_lcv_mj_kg, "LISTING"
    elif market_product in PRODUCT_DEFAULT_LCV:
        lcv, lcv_basis = PRODUCT_DEFAULT_LCV[market_product], "PRODUCT_DEFAULT"
    else:
        return None

    # Compliance-balance improvement per MT of green fuel, in grams:
    # (CI_displaced - CI_g) [g/MJ] * LCV_g [MJ/kg] * 1000 [kg/MT].
    displacement_g_per_mt = max(
        Decimal("0"), (VLSFO_BASELINE_GCO2_MJ - ci) * lcv * KG_PER_MT
    )

    # Marginal penalty rate: 2400 EUR / (GHGIE_actual * 41000) per gram.
    penalty_eur = (
        displacement_g_per_mt
        * PENALTY_EUR_PER_TONNE
        / (ghgie_actual_gco2_mj * VLSFO_MJ_PER_TONNE)
    )
    rate = eur_usd_rate if eur_usd_rate is not None else _effective_eur_usd_rate()

    return ListingOverlay(
        penalty_avoided_eur_per_mt=penalty_eur.quantize(_MONEY, rounding=ROUND_HALF_UP),
        penalty_avoided_usd_per_mt=(penalty_eur * rate).quantize(_MONEY, rounding=ROUND_HALF_UP),
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

    ``fleet_intensity_basis`` labels whether the org has vessels; in both
    cases the prototype computes with GHGIE_actual = 91.16. The basis field
    and vessel count exist so the UI can label the assumption and H1.1 can
    change the number without an API break.
    """
    return OverlayAssumptions(
        eur_usd_rate=_effective_eur_usd_rate(),
        vlsfo_baseline_gco2_mj=VLSFO_BASELINE_GCO2_MJ,
        ghgie_actual_gco2_mj=DEFAULT_GHGIE_ACTUAL_GCO2_MJ,
        fleet_intensity_basis="ORG_FLEET" if fleet_vessel_count > 0 else "DEFAULT_VLSFO",
        fleet_vessel_count=fleet_vessel_count,
        penalty_eur_per_tonne=PENALTY_EUR_PER_TONNE,
        year=year,
        year_target=fueleu_year_target(year),
        excluded_factors=list(EXCLUDED_FACTORS),
    )

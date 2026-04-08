"""
Smart matchmaking service.

Scores compatibility between BID and ASK orders based on:
- Fuel type (must match -- hard filter)
- Region (exact or fuzzy match)
- Price overlap (bid >= ask is ideal)
- Volume compatibility
- Availability window compatibility
"""
from decimal import Decimal, ROUND_HALF_UP

from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window

# Region groupings for fuzzy matching
REGION_GROUPS = {
    "ARA": ["Rotterdam", "Antwerp", "Amsterdam", "ARA"],
    "Singapore": ["Singapore"],
    "Houston": ["Houston", "Gulf Coast"],
    "Fujairah": ["Fujairah", "UAE"],
    "Busan": ["Busan", "South Korea"],
    "Shanghai": ["Shanghai", "China"],
}


def _region_match(bid_region: str, ask_region: str) -> tuple[bool, bool]:
    """Returns (exact_match, fuzzy_match)."""
    if bid_region.lower() == ask_region.lower():
        return True, True

    # Check if both belong to same region group
    for group_regions in REGION_GROUPS.values():
        lower_group = [r.lower() for r in group_regions]
        if bid_region.lower() in lower_group and ask_region.lower() in lower_group:
            return False, True

    return False, False


def _availability_compatible(
    bid_window: str | None,
    ask_window: str | None,
) -> bool:
    """Availability is compatible when both sides target the same canonical bucket."""
    if not bid_window or not ask_window:
        return True
    return normalize_availability_window(bid_window) == normalize_availability_window(ask_window)


def compute_match_score(
    bid_fuel: str, ask_fuel: str,
    bid_region: str, ask_region: str,
    bid_price: Decimal, ask_price: Decimal,
    bid_qty: Decimal, ask_qty: Decimal,
    bid_availability_window: str = SPOT_WINDOW,
    ask_availability_window: str = SPOT_WINDOW,
) -> tuple[Decimal, list[str]]:
    """
    Compute a match score (0-100) and list of match reasons.

    Returns (score, reasons).
    Score of 0 means incompatible (fuel mismatch).
    """
    reasons: list[str] = []

    # Hard filter: fuel type must match
    if bid_fuel.lower() != ask_fuel.lower():
        return Decimal("0"), []

    reasons.append("fuel_type_match")
    score = Decimal("30")  # Base score for fuel match

    # Region matching (0-25 points)
    exact, fuzzy = _region_match(bid_region, ask_region)
    if exact:
        score += Decimal("25")
        reasons.append("region_match")
    elif fuzzy:
        score += Decimal("15")
        reasons.append("region_nearby")

    # Price overlap (0-25 points)
    if bid_price >= ask_price:
        score += Decimal("25")
        reasons.append("price_overlap")
    else:
        # Partial credit for close prices (within 5%)
        gap_pct = (ask_price - bid_price) / ask_price * 100
        if gap_pct <= Decimal("5"):
            score += Decimal("15")
            reasons.append("price_close")
        elif gap_pct <= Decimal("10"):
            score += Decimal("5")
            reasons.append("price_negotiable")

    # Volume compatibility (0-10 points)
    min_qty = min(bid_qty, ask_qty)
    max_qty = max(bid_qty, ask_qty)
    if max_qty > 0:
        ratio = min_qty / max_qty
        if ratio >= Decimal("0.5"):
            score += Decimal("10")
            reasons.append("volume_compatible")
        elif ratio >= Decimal("0.2"):
            score += Decimal("5")
            reasons.append("volume_partial")

    # Availability window (0-10 points)
    if _availability_compatible(bid_availability_window, ask_availability_window):
        score += Decimal("10")
        reasons.append("availability_match")

    return score.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP), reasons

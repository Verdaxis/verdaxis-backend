"""Benchmark-aligned matchmaking scoring."""
from decimal import Decimal, ROUND_HALF_UP

from app.services.availability_windows import normalize_availability_window


def _availability_compatible(
    target_window: str | None,
    candidate_window: str | None,
) -> bool:
    if not target_window or not candidate_window:
        return True
    return normalize_availability_window(target_window) == normalize_availability_window(candidate_window)


def compute_match_score(
    *,
    target_market_product: str | None,
    candidate_market_product: str | None,
    target_delivery_point_id: str | None,
    candidate_delivery_point_id: str | None,
    target_price: Decimal,
    candidate_price: Decimal,
    target_qty: Decimal,
    candidate_qty: Decimal,
    target_availability_window: str | None = None,
    candidate_availability_window: str | None = None,
    candidate_off_spec: bool = False,
    candidate_certification_declared: bool = False,
    candidate_certification_scheme: str | None = None,
    candidate_specification_standard: str | None = None,
    candidate_msds_available: bool = False,
) -> tuple[Decimal, list[str]]:
    """Score a candidate listing against the canonical benchmark market identity."""
    reasons: list[str] = []

    if candidate_off_spec:
        return Decimal("0"), []

    if not target_market_product or not candidate_market_product:
        return Decimal("0"), []

    if target_market_product != candidate_market_product:
        return Decimal("0"), []

    if (
        target_delivery_point_id is not None
        and candidate_delivery_point_id is not None
        and target_delivery_point_id != candidate_delivery_point_id
    ):
        return Decimal("0"), []

    if not _availability_compatible(target_availability_window, candidate_availability_window):
        return Decimal("0"), []

    score = Decimal("40")
    reasons.append("market_product_match")

    if target_delivery_point_id and candidate_delivery_point_id == target_delivery_point_id:
        score += Decimal("20")
        reasons.append("delivery_point_match")
    elif target_delivery_point_id is None and candidate_delivery_point_id:
        score += Decimal("10")
        reasons.append("delivery_point_available")

    if target_availability_window and candidate_availability_window:
        score += Decimal("10")
        reasons.append("availability_match")
    elif candidate_availability_window:
        score += Decimal("5")
        reasons.append("availability_defined")

    if target_price >= candidate_price:
        score += Decimal("20")
        reasons.append("price_overlap")
    else:
        gap_pct = (candidate_price - target_price) / candidate_price * 100
        if gap_pct <= Decimal("5"):
            score += Decimal("10")
            reasons.append("price_close")
        elif gap_pct <= Decimal("10"):
            score += Decimal("5")
            reasons.append("price_negotiable")

    min_qty = min(target_qty, candidate_qty)
    max_qty = max(target_qty, candidate_qty)
    if max_qty > 0:
        ratio = min_qty / max_qty
        if ratio >= Decimal("0.5"):
            score += Decimal("10")
            reasons.append("quantity_fit")
        elif ratio >= Decimal("0.2"):
            score += Decimal("5")
            reasons.append("quantity_partial")

    if (
        candidate_certification_declared
        and candidate_certification_scheme
        and candidate_specification_standard
        and candidate_msds_available
    ):
        score += Decimal("10")
        reasons.append("documentation_complete")
    elif candidate_certification_declared and (
        candidate_certification_scheme or candidate_msds_available or candidate_specification_standard
    ):
        score += Decimal("5")
        reasons.append("documentation_partial")

    return min(score, Decimal("100")).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP), reasons

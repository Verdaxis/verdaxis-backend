"""One B100 policy shared by matching, manual takes and assisted orders."""

from datetime import datetime
from zoneinfo import ZoneInfo

from fastapi import HTTPException
from pydantic import BaseModel, TypeAdapter, ValidationError

from app.market_catalog import PRODUCT_IDS
from app.schemas.fame_order import (
    FameAskTerms,
    FameBidTerms,
    FameOrderTerms,
    FamePublicAskTerms,
    FamePublicTradeSnapshot,
    FameTradeSnapshot,
)

_TERMS = TypeAdapter(FameOrderTerms)


def is_fame_product(product_id) -> bool:
    return str(product_id) == str(PRODUCT_IDS["UCOME_B100"])


def _payload(value):
    return value.model_dump(mode="json") if isinstance(value, BaseModel) else value


def validate_fame_order_terms(product_id, side, terms, availability_window="SPOT"):
    """Validate new/current executable terms without changing other fuels."""
    if not is_fame_product(product_id):
        if terms is not None:
            raise HTTPException(422, "B100 terms are only valid for UCOME B100")
        return None
    try:
        parsed = _TERMS.validate_python(_payload(terms))
    except ValidationError as exc:
        raise HTTPException(
            422, "Valid B100 requirements or declarations are required"
        ) from exc
    if parsed.side != str(getattr(side, "value", side)):
        raise HTTPException(422, "B100 terms must match the order side")
    if isinstance(parsed, FameAskTerms):
        today = datetime.now(ZoneInfo("Asia/Singapore")).date()
        if parsed.certificate_valid_until < today:
            raise HTTPException(422, "Declared operator certificate has expired")
        quality = parsed.quality_evidence
        if quality and any(
            value and value > today for value in (quality.sampled_on, quality.tested_on)
        ):
            raise HTTPException(
                422, "Quality sampling and test dates must not be in the future"
            )
    return parsed.model_dump(mode="json")


def fame_metadata_fields(terms) -> dict:
    """Derive legacy display fields from the single structured declaration."""
    parsed = _TERMS.validate_python(_payload(terms))
    result = {
        "certification_scheme": parsed.sustainability_scheme,
    }
    if isinstance(parsed, FameAskTerms):
        result.update(
            specification_standard=f"{parsed.standard} {parsed.standard_edition}",
            carbon_intensity_gco2_mj=parsed.ci_gco2e_mj,
            carbon_intensity_method=parsed.ci_methodology,
            energy_density_mj_kg=parsed.lhv_mj_kg,
            feedstock="Used cooking oil (100% of feedstock inputs)",
            origin=parsed.production_origin,
        )
    return result


def fame_terms_compatible(bid_terms, ask_terms) -> bool:
    try:
        bid = FameBidTerms.model_validate(_payload(bid_terms))
        ask = FameAskTerms.model_validate(_payload(ask_terms))
        validate_fame_order_terms(PRODUCT_IDS["UCOME_B100"], "ASK", ask)
    except (ValidationError, HTTPException):
        return False
    if (bid.standard, bid.standard_edition, bid.sustainability_scheme) != (
        ask.standard,
        ask.standard_edition,
        ask.sustainability_scheme,
    ):
        return False
    if bid.evidence_due != ask.evidence_due:
        return False
    if any(
        requested is not None and requested != declared
        for requested, declared in (
            (bid.astm_grade, ask.astm_grade),
            (bid.en_climate_class, ask.en_climate_class),
        )
    ):
        return False
    if any(
        limit is not None and (actual is None or actual > limit)
        for limit, actual in (
            (bid.max_cfpp_c, ask.cfpp_c),
            (bid.max_cloud_point_c, ask.cloud_point_c),
            (bid.max_ci_gco2e_mj, ask.ci_gco2e_mj),
        )
    ):
        return False
    if bid.max_ci_gco2e_mj is not None and (
        bid.ci_methodology,
        bid.ci_boundary,
        bid.ci_basis,
    ) != (ask.ci_methodology, ask.ci_boundary, ask.ci_basis):
        return False
    if bid.require_quality_evidence and (
        ask.quality_evidence is None or ask.quality_evidence.status != "AVAILABLE"
    ):
        return False
    sustainability = ask.sustainability_evidence
    if bid.require_sustainability_evidence and (
        sustainability is None or sustainability.status != "AVAILABLE"
    ):
        return False
    return sustainability is None or sustainability.due == bid.evidence_due


def fame_trade_snapshot(bid_terms, ask_terms) -> dict:
    if not fame_terms_compatible(bid_terms, ask_terms):
        raise HTTPException(422, "B100 requirements and declarations are incompatible")
    return FameTradeSnapshot(bid=bid_terms, ask=ask_terms).model_dump(mode="json")


def require_fame_counterparty_terms(order, taker_terms):
    side = str(getattr(order.side, "value", order.side))
    taker_side = "BID" if side == "ASK" else "ASK"
    normalized = validate_fame_order_terms(
        order.product_id, taker_side, taker_terms, order.availability_window
    )
    if normalized is None:
        return None
    source = validate_fame_order_terms(
        order.product_id, side, order.fame_terms, order.availability_window
    )
    return (
        fame_trade_snapshot(normalized, source)
        if side == "ASK"
        else fame_trade_snapshot(source, normalized)
    )


def validate_fame_trade_snapshot(product_id, snapshot, availability_window="SPOT"):
    if not is_fame_product(product_id):
        if snapshot is not None:
            raise HTTPException(422, "B100 snapshots are only valid for UCOME B100")
        return None
    try:
        parsed = FameTradeSnapshot.model_validate(_payload(snapshot))
    except ValidationError as exc:
        raise HTTPException(422, "A complete B100 trade snapshot is required") from exc
    return fame_trade_snapshot(parsed.bid, parsed.ask)


def public_fame_terms(terms):
    if terms is None:
        return None
    parsed = _TERMS.validate_python(_payload(terms))
    if isinstance(parsed, FameBidTerms):
        return parsed.model_dump(mode="json")
    return FamePublicAskTerms.model_validate(parsed.model_dump()).model_dump(
        mode="json"
    )


def redact_fame_trade_snapshot(snapshot, *, reveal_supplier_identity: bool):
    if snapshot is None:
        return None
    parsed = FameTradeSnapshot.model_validate(_payload(snapshot))
    if reveal_supplier_identity:
        return parsed.model_dump(mode="json")
    return FamePublicTradeSnapshot(
        bid=parsed.bid, ask=public_fame_terms(parsed.ask)
    ).model_dump(mode="json")

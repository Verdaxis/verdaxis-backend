"""Bio-LNG declarations, shared execution compatibility and identity redaction."""

from typing import Any

from fastapi import HTTPException
from pydantic import BaseModel, TypeAdapter, ValidationError

from app.market_catalog import PRODUCT_IDS
from app.schemas.lng_order import (
    LngAskTerms, LngBidTerms, LngOrderTerms, LngPublicAskTerms,
    LngPublicTradeSnapshot, LngTradeSnapshot,
)

_TERMS = TypeAdapter(LngOrderTerms)
_SCHEME_LABELS = {"ISCC_EU": "ISCC EU", "REDCERT_EU": "REDcert-EU"}
_FEEDSTOCK_LABELS = {
    "MANURE": "Manure", "FOOD_WASTE": "Food waste", "SEWAGE_SLUDGE": "Sewage sludge",
    "LANDFILL_GAS": "Landfill gas", "AGRICULTURAL_RESIDUES": "Agricultural residues",
    "OTHER_WASTES_AND_RESIDUES": "Other wastes and residues", "ENERGY_CROPS": "Energy crops",
}


def is_lng_product(product_id: Any) -> bool:
    return str(product_id) == str(PRODUCT_IDS["BIO_LNG"])


def _payload(value: Any) -> Any:
    return value.model_dump(mode="json") if isinstance(value, BaseModel) else value


def validate_lng_order_terms(product_id: Any, side: Any, terms: Any) -> dict | None:
    if not is_lng_product(product_id):
        if terms is not None:
            raise HTTPException(422, "LNG terms are only valid for Bio-LNG")
        return None
    try:
        parsed = _TERMS.validate_python(_payload(terms))
    except ValidationError as exc:
        raise HTTPException(422, "Valid Bio-LNG requirements or declarations are required") from exc
    if parsed.side != str(getattr(side, "value", side)):
        raise HTTPException(422, "LNG terms must match the order side")
    return parsed.model_dump(mode="json")


def lng_metadata_fields(terms: Any) -> dict:
    parsed = _TERMS.validate_python(_payload(terms))
    # WTT is not lifecycle CI; never copy it to the generic pricing scalar.
    result = {"carbon_intensity_gco2_mj": None, "carbon_intensity_method": None}
    if isinstance(parsed, LngAskTerms):
        result.update(
            certification_declared=True,
            certification_scheme=_SCHEME_LABELS[parsed.sustainability_scheme],
            specification_standard=parsed.specification_standard,
            feedstock=_FEEDSTOCK_LABELS[parsed.feedstock],
            origin=parsed.production_country,
        )
    return result


def lng_terms_compatible(bid: Any, ask: Any) -> bool:
    """Fail closed on invalid declarations; use the same eight rules everywhere."""
    try:
        bid = LngBidTerms.model_validate(_payload(bid))
        ask = LngAskTerms.model_validate(_payload(ask))
    except ValidationError:
        return False
    return all((
        ask.sustainability_scheme in bid.accepted_schemes,
        ask.chain_of_custody in bid.accepted_custody,
        ask.union_database_registered or not bid.require_union_database,
        bid.min_bio_share_pct is None or ask.bio_share_pct >= bid.min_bio_share_pct,
        bid.max_wtt_ghg_gco2e_mj is None or ask.wtt_ghg_gco2e_mj <= bid.max_wtt_ghg_gco2e_mj,
        ask.delivery_method in bid.accepted_delivery_methods,
        bid.min_methane_number is None or (
            ask.min_methane_number is not None and ask.min_methane_number >= bid.min_methane_number
        ),
        bid.accept_resellers or ask.seller_is_physical_supplier,
    ))


def lng_trade_snapshot(bid: Any, ask: Any) -> dict:
    if not lng_terms_compatible(bid, ask):
        raise HTTPException(422, {"code": "LNG_TERMS_INCOMPATIBLE"})
    return LngTradeSnapshot(bid=bid, ask=ask).model_dump(mode="json")


def require_lng_counterparty_terms(order: Any, taker_terms: Any) -> dict | None:
    side = str(getattr(order.side, "value", order.side))
    normalized = validate_lng_order_terms(order.product_id, "BID" if side == "ASK" else "ASK", taker_terms)
    if normalized is None:
        return None
    source = validate_lng_order_terms(order.product_id, side, getattr(order, "lng_terms", None))
    return lng_trade_snapshot(normalized, source) if side == "ASK" else lng_trade_snapshot(source, normalized)


def validate_lng_trade_snapshot(product_id: Any, snapshot: Any) -> dict | None:
    if not is_lng_product(product_id):
        if snapshot is not None:
            raise HTTPException(422, "LNG snapshots are only valid for Bio-LNG")
        return None
    try:
        parsed = LngTradeSnapshot.model_validate(_payload(snapshot))
    except ValidationError as exc:
        raise HTTPException(422, "A complete Bio-LNG trade snapshot is required") from exc
    return lng_trade_snapshot(parsed.bid, parsed.ask)


def public_lng_terms(terms: Any) -> dict | None:
    if terms is None:
        return None
    parsed = _TERMS.validate_python(_payload(terms))
    if isinstance(parsed, LngBidTerms):
        return parsed.model_dump(mode="json")
    public = parsed.model_dump(exclude={"scheme_certificate_number", "physical_supplier_name"})
    public.update(
        reseller=not parsed.seller_is_physical_supplier,
        fueleu_recognition="UNION_DATABASE" if parsed.union_database_registered else "NOT_REGISTERED",
    )
    return LngPublicAskTerms.model_validate(public).model_dump(mode="json")


def redact_lng_trade_snapshot(snapshot: Any, *, reveal_supplier_identity: bool) -> dict | None:
    if snapshot is None:
        return None
    parsed = LngTradeSnapshot.model_validate(_payload(snapshot))
    if reveal_supplier_identity:
        return parsed.model_dump(mode="json")
    return LngPublicTradeSnapshot(bid=parsed.bid, ask=public_lng_terms(parsed.ask)).model_dump(mode="json")

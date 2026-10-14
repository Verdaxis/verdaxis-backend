"""One Bio-LNG terms policy shared by order entry, matching and direct takes.

Generic order-book predicates (certification, specification, feedstock,
origin) read scalar order columns. For BIO_LNG the server derives those
scalars from ``lng_terms`` only; clients never set them separately.
"""
from __future__ import annotations

from collections.abc import Mapping
from typing import Any
from uuid import UUID

from fastapi import HTTPException, status
from pydantic import BaseModel, TypeAdapter, ValidationError

from app.market_catalog import BIO_LNG_PRODUCT_ID, BIO_LNG_SPECIFICATION_STANDARD
from app.schemas.lng_terms import (
    LngAskTerms,
    LngBidTerms,
    LngOrderTerms,
    LngPublicAskTerms,
    LngPublicTradeSnapshot,
    LngTradeSnapshot,
)

LNG_TERMS_INCOMPATIBLE = "LNG_TERMS_INCOMPATIBLE"

SUSTAINABILITY_SCHEME_LABELS = {
    "ISCC_EU": "ISCC EU",
    "REDCERT_EU": "REDcert-EU",
}
FEEDSTOCK_LABELS = {
    "MANURE": "Manure",
    "FOOD_WASTE": "Food waste",
    "SEWAGE_SLUDGE": "Sewage sludge",
    "LANDFILL_GAS": "Landfill gas",
    "AGRICULTURAL_RESIDUES": "Agricultural residues",
    "OTHER_WASTES_AND_RESIDUES": "Other wastes and residues",
    "ENERGY_CROPS": "Energy crops",
}

_ORDER_TERMS = TypeAdapter(LngOrderTerms)


def is_bio_lng_product(product_id: UUID | str | None) -> bool:
    return product_id is not None and str(product_id) == str(BIO_LNG_PRODUCT_ID)


def _plain(terms: Any) -> Any:
    return terms.model_dump(mode="json") if isinstance(terms, BaseModel) else terms


def _side_value(side: Any) -> str:
    return str(getattr(side, "value", side))


def _terms_error(message: str) -> HTTPException:
    return HTTPException(status_code=status.HTTP_422_UNPROCESSABLE_CONTENT, detail=message)


def validate_order_lng_terms(product_id: UUID | None, side: Any, terms: Any) -> dict | None:
    """Return storable terms for a BIO_LNG order, or None for any other product.

    Terms are required on BIO_LNG orders, must match the order side, and are
    rejected on every other product.
    """
    if not is_bio_lng_product(product_id):
        if terms is not None:
            raise _terms_error("lng_terms are only valid for Bio-LNG orders")
        return None
    if terms is None:
        raise _terms_error("Bio-LNG orders require lng_terms")
    try:
        parsed = _ORDER_TERMS.validate_python(_plain(terms))
    except ValidationError as exc:
        raise _terms_error("lng_terms are not valid Bio-LNG terms") from exc
    if parsed.side != _side_value(side):
        raise _terms_error("lng_terms.side must match the order side")
    return parsed.model_dump(mode="json")


def derived_order_fields(side: Any, terms: Mapping[str, Any]) -> dict[str, object]:
    """Scalar order columns derived from validated BIO_LNG terms.

    The scalar carbon intensity stays NULL: the certified well-to-tank value
    lives only in ``lng_terms`` and must never feed lifecycle CI pricing.
    """
    parsed = _ORDER_TERMS.validate_python(terms)
    if isinstance(parsed, LngBidTerms):
        return {
            "certification_scheme": None,
            "certifications": [
                SUSTAINABILITY_SCHEME_LABELS[scheme] for scheme in parsed.accepted_schemes
            ],
        }
    scheme_label = SUSTAINABILITY_SCHEME_LABELS[parsed.sustainability_scheme]
    return {
        "certification_declared": True,
        "certification_scheme": scheme_label,
        "certifications": [scheme_label],
        "specification_standard": BIO_LNG_SPECIFICATION_STANDARD,
        "feedstock": FEEDSTOCK_LABELS[parsed.feedstock],
        "origin": parsed.production_country,
        "carbon_intensity_gco2_mj": None,
        "carbon_intensity_method": None,
    }


def stored_order_terms_valid(side: Any, terms: Any) -> bool:
    """Fail closed on stored terms that no longer satisfy the current schema."""
    if terms is None:
        return False
    try:
        parsed = _ORDER_TERMS.validate_python(_plain(terms))
    except ValidationError:
        return False
    return parsed.side == _side_value(side)


def lng_terms_compatible(bid: LngBidTerms | Mapping, ask: LngAskTerms | Mapping) -> bool:
    """Return whether an ask meets every bid requirement (design section 3).

    Matching and direct takes both call this function so the two paths
    cannot drift. Invalid or missing terms are never compatible.
    """
    try:
        bid = LngBidTerms.model_validate(_plain(bid))
        ask = LngAskTerms.model_validate(_plain(ask))
    except ValidationError:
        return False
    return all(
        (
            ask.sustainability_scheme in bid.accepted_schemes,
            ask.chain_of_custody in bid.accepted_custody,
            ask.union_database_registered or not bid.require_union_database,
            bid.min_bio_share_pct is None or ask.bio_share_pct >= bid.min_bio_share_pct,
            bid.max_wtt_ghg_gco2e_mj is None
            or ask.wtt_ghg_gco2e_mj <= bid.max_wtt_ghg_gco2e_mj,
            ask.delivery_method in bid.accepted_delivery_methods,
            bid.min_methane_number is None
            or (
                ask.min_methane_number is not None
                and ask.min_methane_number >= bid.min_methane_number
            ),
            bid.accept_resellers or ask.seller_is_physical_supplier,
        )
    )


def lng_terms_incompatible_error() -> HTTPException:
    """Stable rejection for a direct take; it reveals no counterparty detail."""
    return HTTPException(
        status_code=status.HTTP_409_CONFLICT,
        detail={
            "code": LNG_TERMS_INCOMPATIBLE,
            "message": "This Bio-LNG offer does not meet your requirements.",
        },
    )


def lng_trade_snapshot(bid_terms: Any, ask_terms: Any) -> dict:
    """Freeze both sides' terms for a new trade; callers check compatibility first."""
    return LngTradeSnapshot(
        bid=LngBidTerms.model_validate(_plain(bid_terms)),
        ask=LngAskTerms.model_validate(_plain(ask_terms)),
    ).model_dump(mode="json")


def public_ask_terms(ask: LngAskTerms) -> LngPublicAskTerms:
    declaration = ask.model_dump(exclude={"scheme_certificate_number", "physical_supplier_name"})
    return LngPublicAskTerms(
        **declaration,
        reseller=not ask.seller_is_physical_supplier,
        fueleu_recognition=(
            "UNION_DATABASE" if ask.union_database_registered else "NOT_REGISTERED"
        ),
    )


def public_lng_terms(terms: Any) -> Any:
    """Public order view: bids unchanged, asks without seller identity fields."""
    if terms is None:
        return None
    plain = _plain(terms)
    if isinstance(plain, Mapping) and plain.get("side") == "ASK" and "reseller" not in plain:
        return public_ask_terms(LngAskTerms.model_validate(plain)).model_dump(mode="json")
    return plain


def trade_lng_terms_view(snapshot: Any, *, reveal_ask: bool) -> dict | None:
    """Show the full ask only once the trade reveals its counterparties."""
    if snapshot is None:
        return None
    parsed = LngTradeSnapshot.model_validate(_plain(snapshot))
    if reveal_ask:
        return parsed.model_dump(mode="json")
    return LngPublicTradeSnapshot(
        bid=parsed.bid, ask=public_ask_terms(parsed.ask)
    ).model_dump(mode="json")

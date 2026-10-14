"""Public order visibility, shared by the order-book API and read-only market tools.

These predicates are the single source of truth for which orders the public
marketplace shows. ``app.routers.orderbook`` re-exports them under its
historical private names so every existing caller keeps working.

The rules here are deliberately strict: an ASK is public only when it has
declared quality evidence, and only REAL and DEMO organizations are admitted.
"""
from __future__ import annotations

from datetime import datetime

from sqlalchemy import func, or_

from app.market_catalog import PRODUCT_IDS
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.services.market_data_eligibility import (
    active_market_catalog_clauses,
    current_public_order_clause,
    public_order_collection_provenance_clause,
)
from app.services.market_provenance import canonical_availability_window_clause

# Fuel types admitted to the public marketplace. FAME covers UCOME B100.
APPROVED_MARKETPLACE_FUEL_TYPES = ("Methanol", "Ethanol", "FAME", "LNG")


def _ensure_join(joins: list[tuple[object, object]], target: object, condition: object) -> None:
    if not any(existing_target == target for existing_target, _ in joins):
        joins.append((target, condition))


def apply_public_marketplace_scope(
    filters: list[object],
    joins: list[tuple[object, object]],
    *,
    include_off_spec: bool = False,
    as_of: datetime | None = None,
) -> None:
    """Append the public marketplace predicates and their required joins."""
    _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
    _ensure_join(
        joins,
        DeliveryPoint,
        OrderBookOrder.delivery_point_id == DeliveryPoint.id,
    )
    filters.extend(active_market_catalog_clauses(Product, DeliveryPoint))
    filters.append(
        canonical_availability_window_clause(OrderBookOrder.availability_window)
    )
    filters.append(
        current_public_order_clause(OrderBookOrder, now_expression=as_of)
    )
    filters.append(public_order_collection_provenance_clause(OrderBookOrder))
    filters.append(Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES))
    if not include_off_spec:
        filters.append(OrderBookOrder.off_spec.is_(False))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.certification_scheme, ""))) > 0))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.certification_declared.is_(True)))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.specification_standard, ""))) > 0))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.msds_available.is_(True)))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.product_id.in_((PRODUCT_IDS["UCOME_B100"], PRODUCT_IDS["BIO_LNG"])), OrderBookOrder.carbon_intensity_gco2_mj.is_not(None)))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.feedstock, ""))) > 0))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.product_id == PRODUCT_IDS["UCOME_B100"], func.length(func.trim(func.coalesce(OrderBookOrder.origin, ""))) > 0))


def public_order_scope(
    *,
    side: OrderSide | None = None,
    include_off_spec: bool = False,
    as_of: datetime | None = None,
) -> tuple[list[object], list[tuple[object, object]]]:
    """Return the filters and joins that select publicly visible orders."""
    filters: list[object] = [
        OrderBookOrder.status.in_((OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED))
    ]
    if side is not None:
        filters.append(OrderBookOrder.side == side)
    joins: list[tuple[object, object]] = []
    apply_public_marketplace_scope(
        filters,
        joins,
        include_off_spec=include_off_spec,
        as_of=as_of,
    )
    return filters, joins

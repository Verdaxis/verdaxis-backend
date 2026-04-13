from __future__ import annotations

from app.models.orderbook import OrderBookOrder, OrderSide


def normalize_certification_scheme(value: str | None) -> str | None:
    if value is None:
        return None
    normalized = value.strip().upper()
    return normalized or None


def order_is_execution_qualified(order: OrderBookOrder) -> bool:
    if getattr(order, "off_spec", False):
        return False

    if normalize_certification_scheme(getattr(order, "certification_scheme", None)) is None:
        return False

    if getattr(order, "side", None) == OrderSide.ASK and not bool(getattr(order, "certification_declared", False)):
        return False

    return True


def orders_execution_compatible(left: OrderBookOrder, right: OrderBookOrder) -> bool:
    if not order_is_execution_qualified(left) or not order_is_execution_qualified(right):
        return False

    return normalize_certification_scheme(left.certification_scheme) == normalize_certification_scheme(right.certification_scheme)

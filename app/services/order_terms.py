"""Canonical reviewed terms for direct order-book execution."""
from __future__ import annotations

import hashlib
import json
from datetime import UTC, date, datetime
from decimal import Decimal
from enum import Enum
from typing import Any


def _decimal_text(value: Decimal | int | float | None) -> str | None:
    if value is None:
        return None
    decimal_value = value if isinstance(value, Decimal) else Decimal(str(value))
    if decimal_value == 0:
        return "0"
    return format(decimal_value.normalize(), "f")


def _utc_text(value: datetime | None) -> str | None:
    if value is None:
        return None
    if value.tzinfo is None or value.utcoffset() is None:
        value = value.replace(tzinfo=UTC)
    return value.astimezone(UTC).isoformat(timespec="microseconds").replace("+00:00", "Z")


def _enum_value(value: Any) -> Any:
    return value.value if isinstance(value, Enum) else value


def _canonical_json_value(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(key): _canonical_json_value(item) for key, item in sorted(value.items())}
    if isinstance(value, (list, tuple)):
        return [_canonical_json_value(item) for item in value]
    if isinstance(value, Decimal):
        return _decimal_text(value)
    if isinstance(value, datetime):
        return _utc_text(value)
    if isinstance(value, date):
        return value.isoformat()
    if isinstance(value, Enum):
        return value.value
    return value


def executable_order_terms(order: Any) -> dict[str, Any]:
    """Return all order fields a direct-hit review binds.

    Mutable lifecycle state and party admission are checked independently
    under the same transaction locks. This payload binds the order identity,
    displayed economic terms, and execution qualifications.
    """
    certifications = [
        str(value)
        for value in (getattr(order, "certifications", None) or [])
    ]
    return {
        "order_id": str(order.id),
        "side": _enum_value(order.side),
        "organization_id": str(order.organization_id),
        "owner_user_id": str(order.owner_user_id) if order.owner_user_id else None,
        "created_by_actor_user_id": (
            str(order.created_by_actor_user_id)
            if getattr(order, "created_by_actor_user_id", None)
            else None
        ),
        "creation_method": _enum_value(getattr(order, "creation_method", None)),
        "support_authorization_id": (
            str(order.support_authorization_id)
            if getattr(order, "support_authorization_id", None)
            else None
        ),
        "provenance": _enum_value(getattr(order, "provenance", None)),
        "product_id": str(order.product_id),
        "delivery_point_id": str(order.delivery_point_id) if order.delivery_point_id else None,
        "port_id": getattr(order, "port_id", None),
        "vessel_id": str(order.vessel_id) if getattr(order, "vessel_id", None) else None,
        "inventory_item_id": (
            str(order.inventory_item_id)
            if getattr(order, "inventory_item_id", None)
            else None
        ),
        "quantity_mt": _decimal_text(order.quantity_mt),
        "remaining_quantity_mt": _decimal_text(order.remaining_quantity_mt),
        "price_per_mt_usd": _decimal_text(order.price_per_mt_usd),
        "availability_window": order.availability_window,
        "delivery_window_start": _canonical_json_value(
            getattr(order, "delivery_window_start", None)
        ),
        "delivery_window_end": _canonical_json_value(
            getattr(order, "delivery_window_end", None)
        ),
        "expires_at": _utc_text(getattr(order, "expires_at", None)),
        "certifications": certifications,
        "certification_declared": bool(getattr(order, "certification_declared", False)),
        "certification_scheme": getattr(order, "certification_scheme", None),
        "specification_standard": getattr(order, "specification_standard", None),
        "msds_available": bool(getattr(order, "msds_available", False)),
        "carbon_intensity_gco2_mj": _decimal_text(getattr(order, "carbon_intensity_gco2_mj", None)),
        "carbon_intensity_method": getattr(order, "carbon_intensity_method", None),
        "energy_density_mj_kg": _decimal_text(getattr(order, "energy_density_mj_kg", None)),
        "feedstock": getattr(order, "feedstock", None),
        "origin": getattr(order, "origin", None),
        "off_spec": bool(getattr(order, "off_spec", False)),
        "off_spec_notes": getattr(order, "off_spec_notes", None),
        "is_verdaxis_verified": bool(getattr(order, "is_verdaxis_verified", False)),
        "fame_terms": _canonical_json_value(getattr(order, "fame_terms", None)),
    }


def order_terms_digest(order: Any) -> str:
    encoded = json.dumps(
        executable_order_terms(order),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()

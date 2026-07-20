"""Shared validation for economic values crossing the API boundary."""
from __future__ import annotations

from datetime import UTC, datetime
from decimal import Decimal, InvalidOperation


def finite_decimal(value: Decimal | str | int | float, *, field_name: str, scale: int = 2) -> Decimal:
    try:
        parsed = value if isinstance(value, Decimal) else Decimal(str(value))
    except (InvalidOperation, ValueError, TypeError) as exc:
        raise ValueError(f"{field_name} must be a decimal") from exc
    if not parsed.is_finite():
        raise ValueError(f"{field_name} must be finite")
    if -parsed.as_tuple().exponent > scale:
        raise ValueError(f"{field_name} supports at most {scale} decimal places")
    return parsed


def future_aware_datetime(value: datetime | None) -> datetime | None:
    if value is None:
        return None
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError("expires_at must be timezone-aware")
    if value <= datetime.now(UTC):
        raise ValueError("expires_at must be in the future")
    return value

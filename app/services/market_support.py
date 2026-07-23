"""Pure helpers shared by market-support routes and order ownership checks."""
from __future__ import annotations

import hashlib
import json
from datetime import UTC, datetime
from decimal import Decimal
from enum import Enum
from uuid import UUID

from fastapi import HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderBookOrder
from app.models.user import Organization, User
from app.schemas.orderbook import OrderCreate


def _canonical_digest_value(value):
    if isinstance(value, Decimal):
        return format(value.normalize(), "f")
    if isinstance(value, datetime):
        return utc(value).isoformat().replace("+00:00", "Z")
    if isinstance(value, UUID):
        return str(value)
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, dict):
        return {key: _canonical_digest_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_canonical_digest_value(item) for item in value]
    return value


def authorization_terms_digest(order: OrderCreate) -> str:
    payload = _canonical_digest_value(order.model_dump(mode="python"))
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def order_etag(order_id: UUID, version: int) -> str:
    return f'W/"order-{order_id}-v{version}"'


def require_matching_etag(raw: str | None, *, order_id: UUID, version: int) -> None:
    if raw is None:
        raise HTTPException(
            status_code=428,
            detail={"code": "IF_MATCH_REQUIRED", "message": "If-Match is required"},
        )
    value = raw.strip()
    if not value or not value.startswith('W/"') or not value.endswith('"'):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail={"code": "IF_MATCH_MALFORMED", "message": "If-Match is malformed"},
        )
    if value != order_etag(order_id, version):
        raise HTTPException(
            status_code=status.HTTP_412_PRECONDITION_FAILED,
            detail={"code": "ORDER_VERSION_STALE", "message": "Listing changed; reload and retry"},
        )


def utc(value: datetime) -> datetime:
    if value.tzinfo is None or value.utcoffset() is None:
        return value.replace(tzinfo=UTC)
    return value.astimezone(UTC)


async def lock_support_order_parties(db: AsyncSession, order: OrderBookOrder) -> None:
    """Lock the accountable user and organization without requiring eligibility.

    Cancellation must remain available after an account or organization loses
    execution eligibility.
    """
    if order.owner_user_id is not None:
        await db.execute(
            select(User)
            .where(User.id == order.owner_user_id)
            .order_by(User.id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    await db.execute(
        select(Organization)
        .where(Organization.id == order.organization_id)
        .order_by(Organization.id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )

"""Non-impersonating delegated listing workspace for Verdaxis operators."""
from __future__ import annotations

import hashlib
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from typing import Annotated
from uuid import UUID

from fastapi import APIRouter, Depends, Header, HTTPException, Query, Request, status
from slowapi.util import get_remote_address
from sqlalchemy import func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.market_support import (
    AdminCapability,
    AdminCapabilityGrant,
    MarketSupportActionReceipt,
    OrderSupportAttribution,
    OrganizationSupportAuthorization,
    SupportActionOperation,
    SupportAuthorizationScope,
    SupportAuthorizationStatus,
    SupportManagementAuthority,
)
from app.models.notification import NotificationType
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.routers.orderbook import (
    _order_response,
    _supplier_metadata_payload,
    _watchlist_before_state,
    create_order,
    update_order,
)
from app.schemas.market_support import (
    AssistedOrderAttributionResponse,
    AssistedOrderCancelRequest,
    AssistedOrderCreateRequest,
    AssistedOrderPreviewRequest,
    AssistedOrderResponse,
    AssistedOrderUpdateRequest,
    CapabilityGrantCreate,
    CapabilityGrantResponse,
    CapabilityRevokeRequest,
    CustomerAssistedOrderMetadata,
    MarketSupportContextResponse,
    MarketSupportOrderView,
    MarketSupportOrganizationResponse,
    MarketSupportPrincipalResponse,
    PostOnlyPreviewResponse,
    SupportAuthorizationCreate,
    SupportAuthorizationResponse,
    SupportAuthorizationRevokeRequest,
)
from app.schemas.orderbook import OrderCreate, OrderUpdate
from app.services.activity import order_activity_provenance
from app.services.audit_actions import ORDER_CANCELLED
from app.services.audit_service import record_audit, request_audit_context
from app.services.availability_windows import is_tradable_availability_window
from app.services.execution_policy import (
    execution_party_is_eligible,
    normalize_certification_scheme,
)
from app.services.idempotency import acquire_idempotency_lock, idempotency_request_hash
from app.services.inventory_reservations import release_inventory
from app.services.live_benchmarks import LiveBenchmarkKey, rebuild_live_slice_benchmarks_for_keys
from app.services.market_data_eligibility import (
    canonical_delivery_point_clause,
    canonical_market_product_expression,
)
from app.services.market_events import commit_market_events, participant_market_event
from app.services.market_locks import acquire_market_slice_lock, acquire_market_slice_locks
from app.services.market_support_context import (
    MarketSupportActionContext,
    market_support_action_context,
)
from app.services.market_support_policy import (
    bootstrap_authorization_admin_ids,
    delegated_listing_max_ttl_hours,
    delegated_listings_enabled,
)
from app.services.market_support_post_only import PostOnlyAssessment, assess_post_only
from app.services.market_transactions import retry_market_transaction
from app.services.org_notifications import notify_org_users_batched
from app.services.provenance import snapshot_organization_provenance
from app.services.watchlist_events import emit_order_updated


admin_router = APIRouter(prefix="/admin/market-support", tags=["admin-market-support"])
customer_router = APIRouter(prefix="/market-support", tags=["market-support"])

_CREATE_OPERATION = "market_support.order.create"
_UPDATE_OPERATION = "market_support.order.update"
_CANCEL_OPERATION = "market_support.order.cancel"
_ACTIVE_ORDER_STATUSES = (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)

CAPABILITY_GRANTED = "market_support.capability_granted"
CAPABILITY_REVOKED = "market_support.capability_revoked"
AUTHORIZATION_CREATED = "market_support.authorization_created"
AUTHORIZATION_REVOKED = "market_support.authorization_revoked"


def _token_rate_key(request: Request) -> str:
    auth = request.headers.get("Authorization", "")
    if auth:
        return "tok:" + hashlib.sha256(auth.encode("utf-8")).hexdigest()[:16]
    return get_remote_address(request)


def _detail(code: str, message: str, **extra: object) -> dict[str, object]:
    return {"code": code, "message": message, **extra}


def _enum_value(value: object) -> str:
    return str(getattr(value, "value", value))


def _as_utc(value: datetime) -> datetime:
    if value.tzinfo is None or value.utcoffset() is None:
        return value.replace(tzinfo=UTC)
    return value.astimezone(UTC)


def _same_instant(left: datetime, right: datetime) -> bool:
    return _as_utc(left) == _as_utc(right)


def _require_idempotency_key(raw: str | None) -> str:
    key = (raw or "").strip()
    if not key or len(key) > 255:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=_detail(
                "IDEMPOTENCY_KEY_REQUIRED",
                "Idempotency-Key must contain 1-255 characters",
            ),
        )
    return key


def _request_without_idempotency(request: Request) -> Request:
    scope = dict(request.scope)
    scope["headers"] = [
        (name, value)
        for name, value in request.scope.get("headers", [])
        if name.lower() != b"idempotency-key"
    ]
    return Request(scope, receive=request.receive)


async def require_market_support_admin_identity(
    current_user: Annotated[User, Depends(get_current_user)],
) -> User:
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Forbidden")
    return current_user


async def require_market_support_admin(
    current_user: Annotated[
        User, Depends(require_market_support_admin_identity)
    ],
) -> User:
    if not delegated_listings_enabled():
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Not found")
    return current_user


async def _lock_active_capability(
    db: AsyncSession,
    *,
    user_id: UUID,
    capability: AdminCapability,
) -> AdminCapabilityGrant:
    result = await db.execute(
        select(AdminCapabilityGrant)
        .where(
            AdminCapabilityGrant.user_id == user_id,
            AdminCapabilityGrant.capability == capability,
        )
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    grant = result.scalar_one_or_none()
    now = datetime.now(UTC)
    if (
        grant is None
        or grant.revoked_at is not None
        or (grant.expires_at is not None and _as_utc(grant.expires_at) <= now)
    ):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=_detail("MARKET_SUPPORT_CAPABILITY_REQUIRED", "Forbidden"),
        )
    return grant


async def _require_active_capability(
    db: AsyncSession,
    *,
    user_id: UUID,
    capability: AdminCapability,
) -> None:
    now = datetime.now(UTC)
    grant_id = (
        await db.execute(
            select(AdminCapabilityGrant.id).where(
                AdminCapabilityGrant.user_id == user_id,
                AdminCapabilityGrant.capability == capability,
                AdminCapabilityGrant.revoked_at.is_(None),
                or_(
                    AdminCapabilityGrant.expires_at.is_(None),
                    AdminCapabilityGrant.expires_at > now,
                ),
            )
        )
    ).scalar_one_or_none()
    if grant_id is None:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=_detail("MARKET_SUPPORT_CAPABILITY_REQUIRED", "Forbidden"),
        )


async def _active_capabilities(db: AsyncSession, user_id: UUID) -> list[AdminCapability]:
    now = datetime.now(UTC)
    result = await db.execute(
        select(AdminCapabilityGrant.capability).where(
            AdminCapabilityGrant.user_id == user_id,
            AdminCapabilityGrant.revoked_at.is_(None),
            or_(AdminCapabilityGrant.expires_at.is_(None), AdminCapabilityGrant.expires_at > now),
        )
    )
    return list(dict.fromkeys(result.scalars().all()))


async def _require_grant_administrator(
    db: AsyncSession,
    current_user: User,
) -> None:
    if current_user.id in bootstrap_authorization_admin_ids():
        return
    await _lock_active_capability(
        db,
        user_id=current_user.id,
        capability=AdminCapability.MARKET_SUPPORT_AUTHORIZATIONS,
    )


__all__ = [name for name in globals() if not name.startswith("__")]

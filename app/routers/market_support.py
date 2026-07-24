"""Organization-scoped assisted ASK listings without customer impersonation."""
from __future__ import annotations

import hashlib
from datetime import UTC, datetime, timedelta
from typing import Annotated
from uuid import UUID

from fastapi import APIRouter, Depends, Header, HTTPException, Query, Request, Response, status
from sqlalchemy import func, or_, select, text
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.config import settings
from app.database import get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.market_support import (
    MarketSupportAuthorization,
    MarketSupportAuthorizationStatus,
    MarketSupportCapability,
    MarketSupportContext as MarketSupportContextModel,
    MarketSupportContextScope,
    MarketSupportContextStatus,
    StaffCapabilityAssignment,
)
from app.models.notification import NotificationType
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderCreationMethod,
    OrderSide,
)
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from app.routers.auth_simple import get_current_user
from app.routers.orderbook import (
    _best_slice_price,
    _order_response,
    _require_supplier_certification,
    _require_supplier_metadata,
    _watchlist_before_state,
)
from app.schemas.market_support import (
    AssistedListingCancel,
    AssistedListingCreate,
    AssistedListingPage,
    AssistedListingResponse,
    AuthorizationCreate,
    AuthorizationPage,
    AuthorizationResponse,
    AuthorizationRevoke,
    CapabilityAssignmentCreate,
    CapabilityAssignmentResponse,
    CapabilityAssignmentRevoke,
    MarketSupportContext,
    MarketSupportOrganization,
    MarketSupportContextCreate,
    MarketSupportContextResponse,
    MarketSupportEntryResponse,
    MarketSupportPrincipal,
    OrganizationPage,
)
from app.services.activity import order_activity_provenance
from app.services.audit_actions import (
    MARKET_SUPPORT_AUTHORIZATION_CREATED,
    MARKET_SUPPORT_AUTHORIZATION_REVOKED,
    MARKET_SUPPORT_CAPABILITY_GRANTED,
    MARKET_SUPPORT_CAPABILITY_REVOKED,
    MARKET_SUPPORT_CONTEXT_EXITED,
    MARKET_SUPPORT_CONTEXT_REVOKED,
    MARKET_SUPPORT_CONTEXT_STARTED,
    ORDER_CANCELLED,
    ORDER_CREATED,
)
from app.services.audit_service import record_audit, request_audit_context
from app.services.availability_windows import is_tradable_availability_window
from app.services.execution_policy import (
    execution_party_is_eligible,
    normalize_certification_scheme,
)
from app.services.idempotency import (
    acquire_idempotency_lock,
    idempotency_request_hash,
)
from app.services.live_benchmarks import rebuild_live_slice_benchmarks_for_keys
from app.services.market_admission import MarketActorOwnership, lock_and_load_market_organizations
from app.services.market_data_eligibility import (
    canonical_delivery_point_clause,
    canonical_market_product_expression,
)
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.market_locks import acquire_market_slice_lock
from app.services.market_support import (
    authorization_terms_digest,
    lock_support_order_parties,
    order_etag,
    require_matching_etag,
    utc,
)
from app.services.market_support_post_only import assess_locked_order
from app.services.market_transactions import retry_market_transaction
from app.services.org_notifications import notify_org_users_batched
from app.services.provenance import snapshot_organization_provenance
from app.services.watchlist_events import emit_order_created, emit_order_updated


router = APIRouter(prefix="/admin/market-support", tags=["admin-market-support"])
ACTIVE_ORDER_STATUSES = (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
AUTH_CREATE_OPERATION = "market_support.authorization.create"
LISTING_CREATE_OPERATION = "market_support.listing.create"


def _detail(code: str, message: str, **extra: object) -> dict[str, object]:
    return {"code": code, "message": message, **extra}


def _bootstrap_ids() -> frozenset[UUID]:
    values: set[UUID] = set()
    for raw in settings.MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS.split(","):
        try:
            values.add(UUID(raw.strip()))
        except ValueError:
            continue
    return frozenset(values)


async def require_market_support_admin(
    current_user: Annotated[User, Depends(get_current_user)],
) -> User:
    if not settings.MARKET_SUPPORT_ENABLED:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Not found")
    if current_user.role != UserRole.ADMIN or current_user.status != UserStatus.APPROVED:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Forbidden")
    return current_user


async def _lock_capability(
    db: AsyncSession, user_id: UUID, capability: MarketSupportCapability
) -> StaffCapabilityAssignment:
    now = datetime.now(UTC)
    assignment = (
        await db.execute(
            select(StaffCapabilityAssignment)
            .where(
                StaffCapabilityAssignment.user_id == user_id,
                StaffCapabilityAssignment.capability == capability,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    if (
        assignment is None
        or assignment.revoked_at is not None
        or (assignment.expires_at is not None and utc(assignment.expires_at) <= now)
    ):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=_detail("MARKET_SUPPORT_CAPABILITY_REQUIRED", "Forbidden"),
        )
    return assignment


async def _active_capabilities(db: AsyncSession, user_id: UUID) -> list[MarketSupportCapability]:
    now = datetime.now(UTC)
    values = (
        await db.execute(
            select(StaffCapabilityAssignment.capability).where(
                StaffCapabilityAssignment.user_id == user_id,
                StaffCapabilityAssignment.revoked_at.is_(None),
                or_(
                    StaffCapabilityAssignment.expires_at.is_(None),
                    StaffCapabilityAssignment.expires_at > now,
                ),
            )
        )
    ).scalars().all()
    return list(dict.fromkeys(values))


async def _require_any_capability(db: AsyncSession, user_id: UUID) -> None:
    if not await _active_capabilities(db, user_id):
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Forbidden")


async def _require_context_capabilities(db: AsyncSession, user_id: UUID) -> None:
    await _lock_capability(
        db, user_id, MarketSupportCapability.MARKET_SUPPORT_AUTHORIZATIONS
    )
    await _lock_capability(
        db, user_id, MarketSupportCapability.MARKET_SUPPORT_LISTINGS
    )


def _reject_legacy_workspace_mutation() -> None:
    if settings.MARKET_SUPPORT_ENABLED:
        raise HTTPException(
            status_code=status.HTTP_410_GONE,
            detail=_detail(
                "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED",
                "Use an active Market Support context and the normal customer route",
            ),
        )


def _principal_response(user: User) -> MarketSupportPrincipal:
    return MarketSupportPrincipal(
        id=user.id,
        email=user.email,
        name=" ".join(filter(None, [user.first_name, user.last_name])) or user.email,
    )


async def _context_response(
    db: AsyncSession, row: MarketSupportContextModel
) -> MarketSupportContextResponse:
    organization = (
        await db.execute(select(Organization).where(Organization.id == row.organization_id))
    ).scalar_one_or_none()
    actor = (
        await db.execute(select(User).where(User.id == row.actor_user_id))
    ).scalar_one_or_none()
    if organization is None or actor is None:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Support context identities are no longer available",
        )
    return MarketSupportContextResponse(
        id=row.id,
        actor_user_id=row.actor_user_id,
        organization_id=row.organization_id,
        organization=MarketSupportOrganization(
            id=organization.id,
            name=organization.name,
            domain=organization.domain,
            type=str(getattr(organization.type, "value", organization.type)),
        ),
        actor=_principal_response(actor),
        support_reference=row.support_reference,
        scope=row.scope,
        started_at=row.started_at,
        expires_at=row.expires_at,
        ended_at=row.ended_at,
        status=row.status,
        version=row.version,
    )


def _require_bootstrap_operator(user: User) -> None:
    if user.id not in _bootstrap_ids():
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Forbidden")


def _require_idempotency_key(raw: str | None) -> str:
    value = (raw or "").strip()
    if not value or len(value) > 255:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=_detail("IDEMPOTENCY_KEY_REQUIRED", "Idempotency-Key must contain 1-255 characters"),
        )
    return value


async def _authorization_namespace_lock(db: AsyncSession, authorization_id: UUID) -> None:
    if db.get_bind() is None or db.get_bind().dialect.name == "sqlite":
        return
    digest = hashlib.sha256(f"market-support:{authorization_id}".encode()).digest()[:8]
    key = int.from_bytes(digest, "big", signed=True) or 1
    await db.execute(text("SELECT pg_advisory_xact_lock(:key)"), {"key": key})


async def _load_organization(db: AsyncSession, organization_id: UUID) -> Organization:
    organization = (
        await db.execute(select(Organization).where(Organization.id == organization_id))
    ).scalar_one_or_none()
    if organization is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Organization not found")
    if str(organization.verification_status or "").upper() != "APPROVED":
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail("ORGANIZATION_NOT_APPROVED", "Organization is not approved"),
        )
    if str(getattr(organization.provenance, "value", organization.provenance)) != OrganizationProvenance.REAL.value:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail("ORGANIZATION_NOT_REAL", "Market support is limited to real organizations"),
        )
    return organization


async def _load_supplier(
    db: AsyncSession, user_id: UUID, organization: Organization
) -> User:
    user = (await db.execute(select(User).where(User.id == user_id))).scalar_one_or_none()
    if (
        user is None
        or user.organization_id != organization.id
        or user.role != UserRole.SUPPLIER
        or not await execution_party_is_eligible(db, user=user, organization=organization)
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=_detail("ACCOUNTABLE_SUPPLIER_INELIGIBLE", "Select an approved supplier in this organization"),
        )
    return user


async def _load_catalog(db: AsyncSession, product_id: UUID, delivery_point_id: UUID) -> tuple[Product, DeliveryPoint]:
    product = (
        await db.execute(
            select(Product).where(
                Product.id == product_id,
                Product.is_active.is_(True),
                canonical_market_product_expression(Product).is_not(None),
            )
        )
    ).scalar_one_or_none()
    delivery_point = (
        await db.execute(
            select(DeliveryPoint).where(
                DeliveryPoint.id == delivery_point_id,
                canonical_delivery_point_clause(DeliveryPoint),
            )
        )
    ).scalar_one_or_none()
    if product is None or delivery_point is None:
        raise HTTPException(status_code=400, detail="Invalid product or delivery point")
    return product, delivery_point


def _authorization_order(row: MarketSupportAuthorization):
    from app.schemas.orderbook import OrderCreate

    return OrderCreate(
        side=OrderSide(row.order_side),
        product_id=row.product_id,
        delivery_point_id=row.delivery_point_id,
        quantity_mt=row.quantity_mt,
        price_per_mt_usd=row.price_per_mt_usd,
        availability_window=row.availability_window,
        expires_at=row.order_expires_at,
        is_anonymous=row.is_anonymous,
        certifications=list(row.certifications),
        certification_declared=row.certification_declared,
        certification_scheme=row.certification_scheme,
        specification_standard=row.specification_standard,
        msds_available=row.msds_available,
        carbon_intensity_gco2_mj=row.carbon_intensity_gco2_mj,
        carbon_intensity_method=row.carbon_intensity_method,
        feedstock=row.feedstock,
        origin=row.origin,
        off_spec=row.off_spec,
        off_spec_notes=row.off_spec_notes,
    )


def _authorization_response(row: MarketSupportAuthorization) -> AuthorizationResponse:
    return AuthorizationResponse.model_validate(
        {**row.__dict__, "order": _authorization_order(row)}
    )


async def _listing_response(db: AsyncSession, order: OrderBookOrder) -> AssistedListingResponse:
    public_order = await _order_response(db, order)
    return AssistedListingResponse(
        order=public_order,
        accountable_user_id=order.owner_user_id,
        created_by_actor_user_id=order.created_by_actor_user_id,
        creation_method=order.creation_method,
        support_authorization_id=order.support_authorization_id,
        version=order.version,
        etag=order_etag(order.id, order.version),
    )


async def _load_support_order(
    db: AsyncSession, organization_id: UUID, order_id: UUID, *, lock: bool = False
) -> OrderBookOrder:
    statement = (
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(
            OrderBookOrder.id == order_id,
            OrderBookOrder.organization_id == organization_id,
            OrderBookOrder.creation_method == OrderCreationMethod.MARKET_SUPPORT,
            OrderBookOrder.support_authorization_id.is_not(None),
        )
    )
    if lock:
        statement = statement.with_for_update().execution_options(populate_existing=True)
    order = (await db.execute(statement)).scalar_one_or_none()
    if order is None:
        raise HTTPException(status_code=404, detail="Assisted listing not found")
    return order


@router.get("/capabilities", response_model=list[MarketSupportCapability], operation_id="marketSupportCapabilities")
async def capabilities(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    return await _active_capabilities(db, current_user.id)


@router.post(
    "/capability-assignments",
    response_model=CapabilityAssignmentResponse,
    status_code=201,
    operation_id="createMarketSupportCapabilityAssignment",
)
@retry_market_transaction()
async def create_capability_assignment(
    request: Request,
    body: CapabilityAssignmentCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    _require_bootstrap_operator(current_user)
    target = (
        await db.execute(select(User).where(User.id == body.user_id).with_for_update())
    ).scalar_one_or_none()
    if target is None or target.role != UserRole.ADMIN or target.status != UserStatus.APPROVED:
        raise HTTPException(status_code=409, detail="Capability target must be an approved administrator")
    assignment = (
        await db.execute(
            select(StaffCapabilityAssignment)
            .where(
                StaffCapabilityAssignment.user_id == target.id,
                StaffCapabilityAssignment.capability == body.capability,
            )
            .with_for_update()
        )
    ).scalar_one_or_none()
    if assignment is None:
        assignment = StaffCapabilityAssignment(
            user_id=target.id,
            capability=body.capability,
            reason=body.reason,
            granted_by_user_id=current_user.id,
            expires_at=body.expires_at,
        )
        db.add(assignment)
    else:
        assignment.reason = body.reason
        assignment.granted_by_user_id = current_user.id
        assignment.granted_at = datetime.now(UTC)
        assignment.expires_at = body.expires_at
        assignment.revoked_at = None
        assignment.revoked_by_user_id = None
        assignment.revocation_reason = None
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=MARKET_SUPPORT_CAPABILITY_GRANTED,
        resource_type="staff_capability_assignment",
        resource_id=assignment.id,
        changes={"target_user_id": str(target.id), "capability": body.capability.value, "reason": body.reason},
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(assignment)
    return assignment


@router.post(
    "/capability-assignments/{assignment_id}/revoke",
    response_model=CapabilityAssignmentResponse,
    operation_id="revokeMarketSupportCapabilityAssignment",
)
@retry_market_transaction()
async def revoke_capability_assignment(
    assignment_id: UUID,
    request: Request,
    body: CapabilityAssignmentRevoke,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    _require_bootstrap_operator(current_user)
    assignment = (
        await db.execute(
            select(StaffCapabilityAssignment)
            .where(StaffCapabilityAssignment.id == assignment_id)
            .with_for_update()
        )
    ).scalar_one_or_none()
    if assignment is None:
        raise HTTPException(status_code=404, detail="Capability assignment not found")
    # The capability row is locked before any context row.  Every context
    # created for this actor is then revoked in this same transaction, so a
    # request cannot pass its final capability check after revocation commits.
    active_contexts = (
        await db.execute(
            select(MarketSupportContextModel)
            .where(
                MarketSupportContextModel.actor_user_id == assignment.user_id,
                MarketSupportContextModel.status == MarketSupportContextStatus.ACTIVE,
            )
            .order_by(MarketSupportContextModel.id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalars().all()
    if assignment.revoked_at is None:
        assignment.revoked_at = datetime.now(UTC)
        assignment.revoked_by_user_id = current_user.id
        assignment.revocation_reason = body.reason
    await record_audit(
        db,
        user_id=current_user.id,
        action=MARKET_SUPPORT_CAPABILITY_REVOKED,
        resource_type="staff_capability_assignment",
        resource_id=assignment.id,
        changes={"target_user_id": str(assignment.user_id), "capability": assignment.capability.value, "reason": body.reason},
        **request_audit_context(request),
    )
    now = datetime.now(UTC)
    for context in active_contexts:
        context.status = MarketSupportContextStatus.REVOKED
        context.ended_at = now
        context.version += 1
        await record_audit(
            db,
            user_id=current_user.id,
            action=MARKET_SUPPORT_CONTEXT_REVOKED,
            resource_type="market_support_context",
            resource_id=context.id,
            changes={
                "status": context.status.value,
                "reason": body.reason,
                "capability_assignment_id": str(assignment.id),
            },
            **request_audit_context(request),
        )
    await db.commit()
    await db.refresh(assignment)
    return assignment


@router.get("/organizations", response_model=OrganizationPage, operation_id="listMarketSupportOrganizations")
async def organizations(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    query: Annotated[str | None, Query(max_length=100)] = None,
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 25,
):
    await _require_any_capability(db, current_user.id)
    filters = (
        Organization.verification_status == "APPROVED",
        Organization.provenance == OrganizationProvenance.REAL,
    )
    statement = select(Organization).where(*filters)
    count_statement = select(func.count()).select_from(Organization).where(*filters)
    if query and query.strip():
        pattern = f"%{query.strip()}%"
        clause = or_(Organization.name.ilike(pattern), Organization.domain.ilike(pattern))
        statement = statement.where(clause)
        count_statement = count_statement.where(clause)
    rows = (await db.execute(statement.order_by(Organization.name).offset(skip).limit(limit))).scalars().all()
    total = (await db.execute(count_statement)).scalar_one()
    return OrganizationPage(
        items=[MarketSupportOrganization(id=row.id, name=row.name, domain=row.domain, type=str(getattr(row.type, "value", row.type))) for row in rows],
        total=total,
        skip=skip,
        limit=limit,
    )


@router.get(
    "/organizations/{organization_id}/entry",
    response_model=MarketSupportEntryResponse,
    operation_id="marketSupportOrganizationEntry",
)
async def organization_entry(
    organization_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    """Confirm that an approved real organization is eligible for assisted entry."""
    await _require_context_capabilities(db, current_user.id)
    organization = await _load_organization(db, organization_id)
    return MarketSupportEntryResponse(
        organization=MarketSupportOrganization(
            id=organization.id,
            name=organization.name,
            domain=organization.domain,
            type=str(getattr(organization.type, "value", organization.type)),
        ),
        eligible=True,
    )


@router.post(
    "/contexts",
    response_model=MarketSupportContextResponse,
    status_code=status.HTTP_201_CREATED,
    operation_id="createMarketSupportContext",
)
@retry_market_transaction()
async def create_context(
    request: Request,
    body: MarketSupportContextCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_context_capabilities(db, current_user.id)
    organization = await _load_organization(db, body.organization_id)
    now = datetime.now(UTC)
    active_contexts = (
        await db.execute(
            select(MarketSupportContextModel)
            .where(
                MarketSupportContextModel.actor_user_id == current_user.id,
                MarketSupportContextModel.status == MarketSupportContextStatus.ACTIVE,
            )
            .with_for_update()
        )
    ).scalars().all()
    for active in active_contexts:
        if utc(active.expires_at) <= now:
            active.status = MarketSupportContextStatus.EXPIRED
            active.ended_at = now
            active.version += 1
            continue
        if active.organization_id == organization.id:
            return await _context_response(db, active)
        if not body.confirm_replacement:
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail=_detail(
                    "MARKET_SUPPORT_CONTEXT_REPLACEMENT_REQUIRED",
                    "Confirm replacement of the active support context",
                ),
            )
        active.status = MarketSupportContextStatus.EXITED
        active.ended_at = now
        active.version += 1
        await record_audit(
            db,
            user_id=current_user.id,
            action=MARKET_SUPPORT_CONTEXT_EXITED,
            resource_type="market_support_context",
            resource_id=active.id,
            changes={"status": active.status.value, "replacement": True},
            **request_audit_context(request),
        )

    context = MarketSupportContextModel(
        actor_user_id=current_user.id,
        organization_id=organization.id,
        # Legacy column retained for audit compatibility. In organization-scoped
        # contexts it records the accountable administrator, never a proxy customer.
        accountable_user_id=current_user.id,
        support_reference=body.support_reference,
        scope=MarketSupportContextScope.ASSISTED_ORDER_ENTRY,
        started_at=now,
        expires_at=now + timedelta(minutes=settings.MARKET_SUPPORT_CONTEXT_TTL_MINUTES),
        status=MarketSupportContextStatus.ACTIVE,
        version=1,
    )
    db.add(context)
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=MARKET_SUPPORT_CONTEXT_STARTED,
        resource_type="market_support_context",
        resource_id=context.id,
        changes={
            "organization_id": str(organization.id),
            "support_reference": body.support_reference,
            "scope": context.scope.value,
            "expires_at": context.expires_at.isoformat(),
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(context)
    return await _context_response(db, context)


async def _load_owned_context(
    db: AsyncSession, actor_id: UUID, context_id: UUID, *, lock: bool = False
) -> MarketSupportContextModel:
    statement = select(MarketSupportContextModel).where(
        MarketSupportContextModel.id == context_id,
        MarketSupportContextModel.actor_user_id == actor_id,
    )
    if lock:
        statement = statement.with_for_update().execution_options(populate_existing=True)
    context = (await db.execute(statement)).scalar_one_or_none()
    if context is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Not found")
    return context


@router.get(
    "/contexts/active",
    response_model=MarketSupportContextResponse | None,
    operation_id="activeMarketSupportContext",
)
async def active_context(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_context_capabilities(db, current_user.id)
    context = (
        await db.execute(
            select(MarketSupportContextModel)
            .where(
                MarketSupportContextModel.actor_user_id == current_user.id,
                MarketSupportContextModel.status == MarketSupportContextStatus.ACTIVE,
            )
            .order_by(MarketSupportContextModel.started_at.desc())
            .limit(1)
        )
    ).scalar_one_or_none()
    if context is None:
        return None
    if utc(context.expires_at) <= datetime.now(UTC):
        context.status = MarketSupportContextStatus.EXPIRED
        context.ended_at = datetime.now(UTC)
        context.version += 1
        await db.commit()
        return None
    return await _context_response(db, context)


@router.get(
    "/contexts/{context_id}",
    response_model=MarketSupportContextResponse,
    operation_id="getMarketSupportContext",
)
async def get_context(
    context_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_context_capabilities(db, current_user.id)
    context = await _load_owned_context(db, current_user.id, context_id)
    if context.status == MarketSupportContextStatus.ACTIVE and utc(context.expires_at) <= datetime.now(UTC):
        context.status = MarketSupportContextStatus.EXPIRED
        context.ended_at = datetime.now(UTC)
        context.version += 1
        await db.commit()
    return await _context_response(db, context)


@router.post(
    "/contexts/{context_id}/exit",
    response_model=MarketSupportContextResponse,
    operation_id="exitMarketSupportContext",
)
@retry_market_transaction()
async def exit_context(
    context_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_context_capabilities(db, current_user.id)
    context = await _load_owned_context(db, current_user.id, context_id, lock=True)
    if context.status == MarketSupportContextStatus.ACTIVE:
        context.status = (
            MarketSupportContextStatus.EXPIRED
            if utc(context.expires_at) <= datetime.now(UTC)
            else MarketSupportContextStatus.EXITED
        )
        context.ended_at = datetime.now(UTC)
        context.version += 1
        await record_audit(
            db,
            user_id=current_user.id,
            action=MARKET_SUPPORT_CONTEXT_EXITED,
            resource_type="market_support_context",
            resource_id=context.id,
            changes={"status": context.status.value},
            **request_audit_context(request),
        )
        await db.commit()
        await db.refresh(context)
    return await _context_response(db, context)


@router.post(
    "/organizations/{organization_id}/authorizations",
    response_model=AuthorizationResponse,
    status_code=201,
    operation_id="createMarketSupportAuthorization",
)
@retry_market_transaction()
async def create_authorization(
    organization_id: UUID,
    request: Request,
    body: AuthorizationCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    idempotency_key_header: Annotated[str | None, Header(alias="Idempotency-Key")] = None,
):
    _reject_legacy_workspace_mutation()
    await _lock_capability(db, current_user.id, MarketSupportCapability.MARKET_SUPPORT_AUTHORIZATIONS)
    key = _require_idempotency_key(idempotency_key_header)
    request_payload = body.model_dump(mode="json")
    # The confirmation is transient evidence, not an authorization term or
    # part of the pre-context idempotency contract.
    request_payload.get("order", {}).pop("support_confirmation", None)
    request_hash = idempotency_request_hash(request_payload)
    await acquire_idempotency_lock(db, tenant_id=organization_id, operation=AUTH_CREATE_OPERATION, key=key)
    existing = (
        await db.execute(
            select(MarketSupportAuthorization).where(
                MarketSupportAuthorization.organization_id == organization_id,
                MarketSupportAuthorization.idempotency_key == key,
            )
        )
    ).scalar_one_or_none()
    if existing is not None:
        if existing.idempotency_request_hash != request_hash:
            raise HTTPException(status_code=409, detail=_detail("IDEMPOTENCY_KEY_REUSED", "Idempotency-Key was reused with different terms"))
        return _authorization_response(existing)

    now = datetime.now(UTC)
    order_expiry = utc(body.order.expires_at)
    if body.authorization_expires_at <= now or order_expiry <= now:
        raise HTTPException(status_code=409, detail="Authorization and order expiry must be in the future")
    if order_expiry > now + timedelta(hours=settings.MARKET_SUPPORT_MAX_TTL_HOURS):
        raise HTTPException(status_code=409, detail=_detail("MARKET_SUPPORT_TTL_EXCEEDED", "Order expiry exceeds the pilot limit"))
    confirmation = body.order.support_confirmation
    if confirmation is not None and (
        confirmation.instruction_at > now
        or confirmation.instruction_at
        < now - timedelta(hours=settings.MARKET_SUPPORT_MAX_TTL_HOURS)
    ):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=_detail(
                "MARKET_SUPPORT_INSTRUCTION_TIME_INVALID",
                "Instruction time must be recent and not in the future",
            ),
        )
    if not is_tradable_availability_window(body.order.availability_window):
        raise HTTPException(status_code=409, detail="Availability window is not open for new listings")
    scheme = normalize_certification_scheme(body.order.certification_scheme)
    _require_supplier_certification(
        certification_declared=body.order.certification_declared,
        certification_scheme=scheme,
    )
    _require_supplier_metadata(
        specification_standard=body.order.specification_standard.strip(),
        msds_available=body.order.msds_available,
        carbon_intensity_gco2_mj=body.order.carbon_intensity_gco2_mj,
        feedstock=body.order.feedstock,
        origin=body.order.origin,
    )
    organization = await _load_organization(db, organization_id)
    supplier = await _load_supplier(db, body.accountable_user_id, organization)
    await _load_catalog(db, body.order.product_id, body.order.delivery_point_id)
    locked = await lock_and_load_market_organizations(
        db,
        [organization_id],
        actor_ownerships=(MarketActorOwnership(supplier.id, organization_id),),
    )
    organization = locked[organization_id]
    if organization.provenance != OrganizationProvenance.REAL:
        raise HTTPException(status_code=409, detail="Organization is not real")
    row = MarketSupportAuthorization(
        organization_id=organization_id,
        accountable_user_id=supplier.id,
        product_id=body.order.product_id,
        delivery_point_id=body.order.delivery_point_id,
        availability_window=body.order.availability_window,
        quantity_mt=body.order.quantity_mt,
        price_per_mt_usd=body.order.price_per_mt_usd,
        authorization_expires_at=body.authorization_expires_at,
        order_expires_at=order_expiry,
        is_anonymous=body.order.is_anonymous,
        certifications=list(body.order.certifications),
        certification_declared=body.order.certification_declared,
        certification_scheme=scheme,
        specification_standard=body.order.specification_standard,
        msds_available=body.order.msds_available,
        carbon_intensity_gco2_mj=body.order.carbon_intensity_gco2_mj,
        carbon_intensity_method=(body.order.carbon_intensity_method.strip() if body.order.carbon_intensity_method else None),
        feedstock=body.order.feedstock.strip(),
        origin=body.order.origin.strip(),
        off_spec=body.order.off_spec,
        off_spec_notes=(body.order.off_spec_notes.strip() if body.order.off_spec_notes else None),
        terms_digest="0" * 64,
        evidence_reference=body.evidence_reference,
        evidence_sha256=body.evidence_sha256,
        commercial_consent_version=body.commercial_consent_version,
        commercial_consent_reference=body.commercial_consent_reference,
        support_case_reference=body.support_case_reference,
        instruction_at=confirmation.instruction_at if confirmation else None,
        acknowledge_exact_terms=(
            confirmation.acknowledge_exact_terms if confirmation else None
        ),
        acknowledge_executable_standing_order=(
            confirmation.acknowledge_executable_standing_order
            if confirmation
            else None
        ),
        idempotency_key=key,
        idempotency_request_hash=request_hash,
        created_by_actor_user_id=current_user.id,
    )
    row.terms_digest = authorization_terms_digest(_authorization_order(row))
    db.add(row)
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=MARKET_SUPPORT_AUTHORIZATION_CREATED,
        resource_type="market_support_authorization",
        resource_id=row.id,
        changes={
            "organization_id": str(organization_id),
            "accountable_user_id": str(supplier.id),
            "terms_digest": row.terms_digest,
            "evidence_reference": row.evidence_reference,
            "evidence_sha256": row.evidence_sha256,
            "instruction_at": row.instruction_at.isoformat() if row.instruction_at else None,
            "acknowledge_exact_terms": row.acknowledge_exact_terms,
            "acknowledge_executable_standing_order": row.acknowledge_executable_standing_order,
            "commercial_consent_version": row.commercial_consent_version,
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(row)
    return _authorization_response(row)


@router.get(
    "/organizations/{organization_id}/authorizations",
    response_model=AuthorizationPage,
    operation_id="listMarketSupportAuthorizations",
)
async def list_authorizations(
    organization_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 25,
):
    await _require_any_capability(db, current_user.id)
    await _load_organization(db, organization_id)
    filters = (MarketSupportAuthorization.organization_id == organization_id,)
    rows = (
        await db.execute(
            select(MarketSupportAuthorization)
            .where(*filters)
            .order_by(MarketSupportAuthorization.created_at.desc())
            .offset(skip)
            .limit(limit)
        )
    ).scalars().all()
    total = (
        await db.execute(select(func.count()).select_from(MarketSupportAuthorization).where(*filters))
    ).scalar_one()
    return AuthorizationPage(items=[_authorization_response(row) for row in rows], total=total, skip=skip, limit=limit)


def _candidate_from_authorization(
    authorization: MarketSupportAuthorization,
    organization: Organization,
    product: Product,
    delivery_point: DeliveryPoint,
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=authorization.organization_id,
        owner_user_id=authorization.accountable_user_id,
        created_by_actor_user_id=authorization.created_by_actor_user_id,
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
        support_authorization_id=authorization.id,
        provenance=snapshot_organization_provenance(organization),
        side=OrderSide.ASK,
        product_id=authorization.product_id,
        delivery_point_id=authorization.delivery_point_id,
        quantity_mt=authorization.quantity_mt,
        remaining_quantity_mt=authorization.quantity_mt,
        price_per_mt_usd=authorization.price_per_mt_usd,
        availability_window=authorization.availability_window,
        expires_at=authorization.order_expires_at,
        certifications=list(authorization.certifications),
        certification_declared=authorization.certification_declared,
        certification_scheme=authorization.certification_scheme,
        specification_standard=authorization.specification_standard,
        msds_available=authorization.msds_available,
        carbon_intensity_gco2_mj=authorization.carbon_intensity_gco2_mj,
        carbon_intensity_method=authorization.carbon_intensity_method,
        feedstock=authorization.feedstock,
        origin=authorization.origin,
        off_spec=authorization.off_spec,
        off_spec_notes=authorization.off_spec_notes,
    )
    return order


@router.post(
    "/organizations/{organization_id}/listings",
    response_model=AssistedListingResponse,
    status_code=201,
    operation_id="createMarketSupportListing",
)
@retry_market_transaction()
async def create_listing(
    organization_id: UUID,
    request: Request,
    response: Response,
    body: AssistedListingCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    idempotency_key_header: Annotated[str | None, Header(alias="Idempotency-Key")] = None,
):
    _reject_legacy_workspace_mutation()
    await _lock_capability(db, current_user.id, MarketSupportCapability.MARKET_SUPPORT_LISTINGS)
    key = _require_idempotency_key(idempotency_key_header)
    request_hash = idempotency_request_hash(body.model_dump(mode="json"))
    await acquire_idempotency_lock(db, tenant_id=organization_id, operation=LISTING_CREATE_OPERATION, key=key)
    replay = (
        await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.organization), selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(
                OrderBookOrder.organization_id == organization_id,
                OrderBookOrder.idempotency_operation == LISTING_CREATE_OPERATION,
                OrderBookOrder.idempotency_key == key,
            )
        )
    ).scalar_one_or_none()
    if replay is not None:
        if replay.idempotency_request_hash != request_hash:
            raise HTTPException(status_code=409, detail=_detail("IDEMPOTENCY_KEY_REUSED", "Idempotency-Key was reused with a different authorization"))
        result = await _listing_response(db, replay)
        response.headers["ETag"] = result.etag
        return result

    await _authorization_namespace_lock(db, body.authorization_id)
    preview = (
        await db.execute(
            select(MarketSupportAuthorization).where(
                MarketSupportAuthorization.id == body.authorization_id,
                MarketSupportAuthorization.organization_id == organization_id,
            )
        )
    ).scalar_one_or_none()
    if preview is None:
        raise HTTPException(status_code=404, detail="Support authorization not found")
    await acquire_market_slice_lock(
        db,
        side=OrderSide.ASK,
        product_id=preview.product_id,
        delivery_point_id=preview.delivery_point_id,
        availability_window=preview.availability_window,
    )
    authorization = (
        await db.execute(
            select(MarketSupportAuthorization)
            .where(
                MarketSupportAuthorization.id == body.authorization_id,
                MarketSupportAuthorization.organization_id == organization_id,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    now = datetime.now(UTC)
    if authorization is None:
        raise HTTPException(status_code=404, detail="Support authorization not found")
    if authorization.status != MarketSupportAuthorizationStatus.ACTIVE:
        raise HTTPException(status_code=409, detail=_detail("SUPPORT_AUTHORIZATION_NOT_ACTIVE", "Authorization has already been consumed or revoked"))
    if utc(authorization.authorization_expires_at) <= now or utc(authorization.order_expires_at) <= now:
        raise HTTPException(status_code=409, detail=_detail("SUPPORT_AUTHORIZATION_EXPIRED", "Authorization has expired"))
    if authorization_terms_digest(_authorization_order(authorization)) != authorization.terms_digest:
        raise HTTPException(
            status_code=409,
            detail=_detail("SUPPORT_AUTHORIZATION_TERMS_MISMATCH", "Stored authorization terms failed integrity validation"),
        )
    organization = await _load_organization(db, organization_id)
    supplier = await _load_supplier(db, authorization.accountable_user_id, organization)
    product, delivery_point = await _load_catalog(db, authorization.product_id, authorization.delivery_point_id)
    candidate = _candidate_from_authorization(authorization, organization, product, delivery_point)
    assessment = await assess_locked_order(db, candidate, organization=organization)
    if assessment.indeterminate:
        raise HTTPException(status_code=409, detail=_detail("POST_ONLY_CHECK_INDETERMINATE", "The complete crossing set could not be assessed; retry later"))
    if assessment.would_cross:
        raise HTTPException(
            status_code=409,
            detail=_detail(
                "POST_ONLY_WOULD_CROSS",
                "Assisted publication must rest without immediate execution",
                best_executable_opposing_price=(str(assessment.best_executable_price) if assessment.best_executable_price is not None else None),
            ),
        )
    previous_best = await _best_slice_price(
        db,
        market_product_code=product.market_product,
        delivery_point_id=authorization.delivery_point_id,
        availability_window_code=authorization.availability_window,
        side=OrderSide.ASK,
    )
    candidate.organization = organization
    candidate.product = product
    candidate.delivery_point = delivery_point
    candidate.created_by_actor_user_id = current_user.id
    candidate.idempotency_key = key
    candidate.idempotency_operation = LISTING_CREATE_OPERATION
    candidate.idempotency_request_hash = request_hash
    db.add(candidate)
    authorization.status = MarketSupportAuthorizationStatus.CONSUMED
    authorization.consumed_at = now
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=ORDER_CREATED,
        resource_type="order",
        resource_id=candidate.id,
        changes={
            "organization_id": str(organization_id),
            "accountable_user_id": str(supplier.id),
            "support_authorization_id": str(authorization.id),
            "creation_method": OrderCreationMethod.MARKET_SUPPORT.value,
            "terms_digest": authorization.terms_digest,
        },
        **request_audit_context(request),
    )
    await notify_org_users_batched(
        db,
        [(organization_id, NotificationType.ORDER_UPDATE, "Listing set up by Verdaxis Support", "Verdaxis Support published an authorized listing for your organization.", {"order_id": str(candidate.id), "creation_method": OrderCreationMethod.MARKET_SUPPORT.value})],
    )
    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [(OrderSide.ASK, product.market_product, authorization.delivery_point_id, authorization.availability_window)],
    )
    await emit_order_created(db, candidate, previous_best_price=previous_best)
    await enqueue_market_events(
        db,
        [participant_market_event(
            event_type="order_created",
            aggregate_type="order",
            aggregate_id=candidate.id,
            participant_org_ids=(organization_id,),
            payload={
                **order_activity_provenance(candidate),
                "id": str(candidate.id),
                "side": candidate.side.value,
                "product_name": candidate.product_name,
                "fuel_type": candidate.fuel_type,
                "region": candidate.region,
                "price": str(candidate.price_per_mt_usd),
                "quantity": str(candidate.remaining_quantity_mt),
            },
        )],
    )
    await db.commit()
    order = await _load_support_order(db, organization_id, candidate.id)
    result = await _listing_response(db, order)
    response.headers["ETag"] = result.etag
    return result


@router.get(
    "/organizations/{organization_id}/listings",
    response_model=AssistedListingPage,
    operation_id="listMarketSupportListings",
)
async def list_listings(
    organization_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 25,
):
    await _lock_capability(db, current_user.id, MarketSupportCapability.MARKET_SUPPORT_LISTINGS)
    await _load_organization(db, organization_id)
    filters = (
        OrderBookOrder.organization_id == organization_id,
        OrderBookOrder.creation_method == OrderCreationMethod.MARKET_SUPPORT,
    )
    rows = (
        await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.organization), selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(*filters)
            .order_by(OrderBookOrder.created_at.desc())
            .offset(skip)
            .limit(limit)
        )
    ).scalars().all()
    total = (await db.execute(select(func.count()).select_from(OrderBookOrder).where(*filters))).scalar_one()
    return AssistedListingPage(items=[await _listing_response(db, row) for row in rows], total=total, skip=skip, limit=limit)


async def _cancel_locked_order(
    db: AsyncSession,
    *,
    order: OrderBookOrder,
    actor: User,
    reason: str,
    request: Request,
) -> None:
    before = await _watchlist_before_state(db, order)
    order.status = OrderBookStatus.CANCELLED
    order.version += 1
    await record_audit(
        db,
        user_id=actor.id,
        action=ORDER_CANCELLED,
        resource_type="order",
        resource_id=order.id,
        changes={
            "organization_id": str(order.organization_id),
            "support_authorization_id": str(order.support_authorization_id),
            "reason": reason,
            "version": order.version,
        },
        **request_audit_context(request),
    )
    await notify_org_users_batched(
        db,
        [(order.organization_id, NotificationType.ORDER_UPDATE, "Listing cancelled by Verdaxis Support", "Verdaxis Support cancelled the remaining quantity of an assisted listing.", {"order_id": str(order.id), "creation_method": OrderCreationMethod.MARKET_SUPPORT.value})],
    )
    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [(order.side, order.market_product, order.delivery_point_id, order.availability_window)],
    )
    await emit_order_updated(db, before=before, order=order)
    await enqueue_market_events(
        db,
        [participant_market_event(
            event_type="order_cancelled",
            aggregate_type="order",
            aggregate_id=order.id,
            participant_org_ids=(order.organization_id,),
            payload={**order_activity_provenance(order), "id": str(order.id), "side": order.side.value, "product_name": order.product_name, "fuel_type": order.fuel_type, "region": order.region},
        )],
    )


@router.post(
    "/organizations/{organization_id}/listings/{order_id}/cancel",
    response_model=AssistedListingResponse,
    operation_id="cancelMarketSupportListing",
)
@retry_market_transaction()
async def cancel_listing(
    organization_id: UUID,
    order_id: UUID,
    request: Request,
    response: Response,
    body: AssistedListingCancel,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
    if_match: Annotated[str | None, Header(alias="If-Match")] = None,
):
    _reject_legacy_workspace_mutation()
    await _lock_capability(db, current_user.id, MarketSupportCapability.MARKET_SUPPORT_LISTINGS)
    preview = await _load_support_order(db, organization_id, order_id)
    require_matching_etag(if_match, order_id=order_id, version=preview.version)
    await acquire_market_slice_lock(db, side=preview.side, product_id=preview.product_id, delivery_point_id=preview.delivery_point_id, availability_window=preview.availability_window)
    order = await _load_support_order(db, organization_id, order_id, lock=True)
    require_matching_etag(if_match, order_id=order_id, version=order.version)
    await lock_support_order_parties(db, order)
    if order.status in ACTIVE_ORDER_STATUSES:
        await _cancel_locked_order(db, order=order, actor=current_user, reason=body.reason, request=request)
        await db.commit()
        order = await _load_support_order(db, organization_id, order_id)
    result = await _listing_response(db, order)
    response.headers["ETag"] = result.etag
    return result


@router.post(
    "/organizations/{organization_id}/authorizations/{authorization_id}/revoke",
    response_model=AuthorizationResponse,
    operation_id="revokeMarketSupportAuthorization",
)
@retry_market_transaction()
async def revoke_authorization(
    organization_id: UUID,
    authorization_id: UUID,
    request: Request,
    body: AuthorizationRevoke,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    _reject_legacy_workspace_mutation()
    await _lock_capability(db, current_user.id, MarketSupportCapability.MARKET_SUPPORT_AUTHORIZATIONS)
    await _authorization_namespace_lock(db, authorization_id)
    preview_order = (
        await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point), selectinload(OrderBookOrder.organization))
            .where(
                OrderBookOrder.organization_id == organization_id,
                OrderBookOrder.support_authorization_id == authorization_id,
            )
        )
    ).scalar_one_or_none()
    if preview_order is not None:
        await acquire_market_slice_lock(db, side=preview_order.side, product_id=preview_order.product_id, delivery_point_id=preview_order.delivery_point_id, availability_window=preview_order.availability_window)
    authorization = (
        await db.execute(
            select(MarketSupportAuthorization)
            .where(MarketSupportAuthorization.id == authorization_id, MarketSupportAuthorization.organization_id == organization_id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    if authorization is None:
        raise HTTPException(status_code=404, detail="Support authorization not found")
    if authorization.status == MarketSupportAuthorizationStatus.REVOKED:
        return _authorization_response(authorization)
    order = None
    if preview_order is not None:
        order = await _load_support_order(db, organization_id, preview_order.id, lock=True)
        await lock_support_order_parties(db, order)
    authorization.status = MarketSupportAuthorizationStatus.REVOKED
    authorization.revoked_at = datetime.now(UTC)
    authorization.revoked_by_actor_user_id = current_user.id
    authorization.revocation_reason = body.reason
    if order is not None and order.status in ACTIVE_ORDER_STATUSES:
        await _cancel_locked_order(db, order=order, actor=current_user, reason="Authorization revoked: " + body.reason, request=request)
    await record_audit(
        db,
        user_id=current_user.id,
        action=MARKET_SUPPORT_AUTHORIZATION_REVOKED,
        resource_type="market_support_authorization",
        resource_id=authorization.id,
        changes={"organization_id": str(organization_id), "reason": body.reason, "linked_order_id": str(order.id) if order else None},
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(authorization)
    return _authorization_response(authorization)


@router.get(
    "/organizations/{organization_id}/context",
    response_model=MarketSupportContext,
    operation_id="getMarketSupportOrganizationContext",
)
async def organization_context(
    organization_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_market_support_admin)],
):
    await _require_any_capability(db, current_user.id)
    organization = await _load_organization(db, organization_id)
    supplier_rows = (
        await db.execute(
            select(User)
            .where(User.organization_id == organization_id, User.role == UserRole.SUPPLIER)
            .order_by(User.email)
        )
    ).scalars().all()
    principals = []
    for user in supplier_rows:
        if await execution_party_is_eligible(db, user=user, organization=organization):
            name = " ".join(value for value in (user.first_name, user.last_name) if value).strip() or user.email
            principals.append(MarketSupportPrincipal(id=user.id, email=user.email, name=name))
    authorizations = (
        await db.execute(
            select(MarketSupportAuthorization)
            .where(MarketSupportAuthorization.organization_id == organization_id)
            .order_by(MarketSupportAuthorization.created_at.desc())
            .limit(100)
        )
    ).scalars().all()
    orders = (
        await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.organization), selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(OrderBookOrder.organization_id == organization_id, OrderBookOrder.creation_method == OrderCreationMethod.MARKET_SUPPORT)
            .order_by(OrderBookOrder.created_at.desc())
            .limit(100)
        )
    ).scalars().all()
    return MarketSupportContext(
        organization=MarketSupportOrganization(id=organization.id, name=organization.name, domain=organization.domain, type=str(getattr(organization.type, "value", organization.type))),
        eligible_principals=principals,
        authorizations=[_authorization_response(row) for row in authorizations],
        listings=[await _listing_response(db, row) for row in orders],
    )

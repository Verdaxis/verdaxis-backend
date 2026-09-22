"""Supplier indications share marketplace presentation, never trade execution."""

from datetime import UTC, datetime
from typing import Annotated, Literal
from uuid import UUID

from fastapi import APIRouter, Depends, Header, HTTPException, Query, Request
from sqlalchemy import func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.market_catalog import PRODUCT_IDS
from app.models.supplier_offer import SupplierOffer
from app.models.user import Organization, OrganizationProvenance, User, UserRole
from app.rate_limit import limiter
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.schemas.supplier_offer import (
    SupplierOfferCreate,
    SupplierOfferList,
    SupplierOfferResponse,
    SupplierOfferUpdate,
    SupplierOfferWithdraw,
)
from app.services.audit_actions import (
    SUPPLIER_OFFER_CREATED,
    SUPPLIER_OFFER_UPDATED,
    SUPPLIER_OFFER_WITHDRAWN,
)
from app.services.audit_service import record_audit, request_audit_context
from app.services.execution_policy import execution_party_is_eligible
from app.services.idempotency import acquire_idempotency_lock, idempotency_request_hash
from app.services.market_admission import (
    MarketActorOwnership,
    assert_market_participant_provenance,
    lock_and_load_market_organizations,
)
from app.services.market_catalog_validation import require_canonical_market_slice
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.market_locks import acquire_market_slice_locks
from app.services.market_transactions import retry_market_transaction
from app.services.security_market_admission import require_security_market_admission
from app.services.supplier_offers import (
    listing_terms_payload,
    offer_response,
    offer_snapshot,
    public_offer_clause,
    validate_offer_dates,
)


router = APIRouter(prefix="/supplier-offers", tags=["supplier-offers"])
OfferSort = Literal["newest", "price_asc", "price_desc", "quantity_desc"]
Database = Annotated[AsyncSession, Depends(get_db)]
Reader = Annotated[User, Depends(get_authenticated_user)]
Publisher = Annotated[User, Depends(get_current_user)]
Admission = Annotated[None, Depends(require_security_market_admission)]


def _require_supplier(user: User) -> None:
    if user.role != UserRole.SUPPLIER or user.organization_id is None:
        raise HTTPException(
            status_code=403,
            detail="Only suppliers in an organization can publish offers",
        )


def _owned(offer: SupplierOffer, user: User) -> bool:
    return (
        offer.supplier_org_id == user.organization_id
        and offer.supplier_user_id == user.id
    )


def _offer_key(offer: SupplierOffer):
    return ("ASK", offer.product_id, offer.delivery_point_id, offer.availability_window)


async def _validate_catalog(db: AsyncSession, payload: SupplierOfferCreate) -> None:
    if payload.product_id != PRODUCT_IDS["UCOME_B100"]:
        raise HTTPException(
            status_code=422, detail="Supplier offers currently support UCOME B100 only"
        )
    await require_canonical_market_slice(
        db, product_id=payload.product_id, delivery_point_id=payload.delivery_point_id
    )


async def _lock_publisher(
    db: AsyncSession, user: User, *, retract: bool = False
) -> None:
    if user.organization_id is None:
        raise HTTPException(status_code=403, detail="An organization is required")
    organizations = await lock_and_load_market_organizations(
        db,
        [user.organization_id],
        actor_ownerships=(MarketActorOwnership(user.id, user.organization_id),),
        require_approved=not retract,
        require_execution_eligible=not retract,
    )
    if not retract:
        _require_supplier(user)
        assert_market_participant_provenance(organizations[user.organization_id])


async def _load_owned_locked(
    db: AsyncSession,
    offer_id: UUID,
    user: User,
    *,
    next_payload: SupplierOfferCreate | None = None,
) -> SupplierOffer:
    offer = (
        await db.execute(select(SupplierOffer).where(SupplierOffer.id == offer_id))
    ).scalar_one_or_none()
    if offer is None or not _owned(offer, user):
        raise HTTPException(status_code=404, detail="Supplier offer not found")
    original_key = _offer_key(offer)
    keys = [original_key]
    if next_payload:
        keys.append(
            (
                "ASK",
                next_payload.product_id,
                next_payload.delivery_point_id,
                next_payload.availability_window,
            )
        )
    await acquire_market_slice_locks(db, keys)
    offer = (
        await db.execute(
            select(SupplierOffer)
            .where(SupplierOffer.id == offer_id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one()
    if _offer_key(offer) != original_key or not _owned(offer, user):
        raise HTTPException(
            status_code=409,
            detail="Supplier offer changed; reload before making changes",
        )
    return offer


async def _commit_event(
    db: AsyncSession, offer: SupplierOffer, event_type: str
) -> None:
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type=event_type,
                aggregate_type="supplier_offer",
                aggregate_id=offer.id,
                participant_org_ids=(offer.supplier_org_id,),
                payload={
                    "offer_id": str(offer.id),
                    "revision": offer.revision,
                    "status": offer.status,
                },
            )
        ],
    )
    await db.commit()


async def _viewer_state(db: AsyncSession, user: User) -> dict[str, bool]:
    organization = (
        await db.execute(
            select(Organization).where(Organization.id == user.organization_id)
        )
    ).scalar_one_or_none()
    eligible = bool(organization) and await execution_party_is_eligible(
        db, user=user, organization=organization
    )
    return {
        "viewer_is_eligible": eligible,
        "viewer_can_inquire": eligible
        and user.role == UserRole.BUYER
        and organization.provenance == OrganizationProvenance.REAL,
    }


async def _list_offers(
    db: AsyncSession,
    user: User,
    *,
    own: bool,
    product_id: UUID | None,
    delivery_point_id: UUID | None,
    availability_window: str | None,
    region: str | None,
    status_filter: str | None,
    sort_by: OfferSort,
    skip: int,
    limit: int,
) -> SupplierOfferList:
    if own:
        if user.organization_id is None:
            return SupplierOfferList(items=[], total=0, skip=skip, limit=limit)
        filters = [SupplierOffer.supplier_org_id == user.organization_id]
    else:
        filters = [public_offer_clause()]
    if product_id:
        filters.append(SupplierOffer.product_id == product_id)
    if delivery_point_id:
        filters.append(SupplierOffer.delivery_point_id == delivery_point_id)
    if availability_window:
        filters.append(SupplierOffer.availability_window == availability_window)
    if region and region not in ("Singapore", "Asia", "Asia Pacific"):
        return SupplierOfferList(items=[], total=0, skip=skip, limit=limit)
    if status_filter == "EXPIRED":
        filters.extend(
            (
                SupplierOffer.status == "OPEN",
                SupplierOffer.expires_at <= datetime.now(UTC),
            )
        )
    elif status_filter == "OPEN":
        filters.extend(
            (
                SupplierOffer.status == "OPEN",
                SupplierOffer.expires_at > datetime.now(UTC),
            )
        )
    elif status_filter == "WITHDRAWN":
        filters.append(SupplierOffer.status == "WITHDRAWN")
    total = (
        await db.execute(select(func.count(SupplierOffer.id)).where(*filters))
    ).scalar_one()
    sort = {
        "newest": SupplierOffer.created_at.desc(),
        "price_asc": SupplierOffer.price_per_mt_usd.asc(),
        "price_desc": SupplierOffer.price_per_mt_usd.desc(),
        "quantity_desc": SupplierOffer.quantity_mt.desc(),
    }[sort_by]
    offers = (
        (
            await db.execute(
                select(SupplierOffer)
                .where(*filters)
                .order_by(sort, SupplierOffer.id)
                .offset(skip)
                .limit(limit)
            )
        )
        .scalars()
        .all()
    )
    viewer_state = await _viewer_state(db, user)
    return SupplierOfferList(
        items=[offer_response(offer, user, **viewer_state) for offer in offers],
        total=total,
        skip=skip,
        limit=limit,
    )


@router.get("", response_model=SupplierOfferList)
async def list_offers(
    db: Database,
    current_user: Reader,
    product_id: UUID | None = None,
    delivery_point_id: UUID | None = None,
    availability_window: str | None = None,
    region: str | None = None,
    sort_by: OfferSort = "newest",
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
):
    return await _list_offers(
        db,
        current_user,
        own=False,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window=availability_window,
        region=region,
        status_filter=None,
        sort_by=sort_by,
        skip=skip,
        limit=limit,
    )


@router.get("/my", response_model=SupplierOfferList)
async def list_my_offers(
    db: Database,
    current_user: Reader,
    product_id: UUID | None = None,
    delivery_point_id: UUID | None = None,
    availability_window: str | None = None,
    region: str | None = None,
    status_filter: Literal["OPEN", "WITHDRAWN", "EXPIRED"] | None = Query(
        None, alias="status"
    ),
    sort_by: OfferSort = "newest",
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
):
    return await _list_offers(
        db,
        current_user,
        own=True,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window=availability_window,
        region=region,
        status_filter=status_filter,
        sort_by=sort_by,
        skip=skip,
        limit=limit,
    )


@router.get("/{offer_id}", response_model=SupplierOfferResponse)
async def get_offer(offer_id: UUID, db: Database, current_user: Reader):
    offer = (
        await db.execute(
            select(SupplierOffer).where(
                SupplierOffer.id == offer_id,
                or_(
                    SupplierOffer.supplier_org_id == current_user.organization_id,
                    public_offer_clause(),
                ),
            )
        )
    ).scalar_one_or_none()
    if offer is None:
        raise HTTPException(status_code=404, detail="Supplier offer not found")
    return offer_response(offer, current_user, **await _viewer_state(db, current_user))


@router.post("", response_model=SupplierOfferResponse, status_code=201)
@limiter.limit("30/minute")
@retry_market_transaction()
async def create_offer(
    request: Request,
    payload: SupplierOfferCreate,
    db: Database,
    current_user: Publisher,
    _admission: Admission,
    idempotency_key: str = Header(
        ..., alias="Idempotency-Key", min_length=1, max_length=255
    ),
):
    _require_supplier(current_user)
    key = idempotency_key.strip()
    if not key:
        raise HTTPException(status_code=422, detail="Idempotency-Key cannot be blank")
    digest = idempotency_request_hash(
        {"actor_id": str(current_user.id), "offer": payload.model_dump(mode="json")}
    )
    await acquire_idempotency_lock(
        db,
        tenant_id=current_user.organization_id,
        operation="supplier_offer.create",
        key=key,
    )
    existing = (
        await db.execute(
            select(SupplierOffer).where(
                SupplierOffer.supplier_org_id == current_user.organization_id,
                SupplierOffer.idempotency_key == key,
            )
        )
    ).scalar_one_or_none()
    if existing:
        if existing.idempotency_request_hash != digest or not _owned(
            existing, current_user
        ):
            raise HTTPException(
                status_code=409,
                detail="Idempotency-Key was used for another offer request",
            )
        return offer_response(existing, current_user, viewer_is_eligible=True)
    validate_offer_dates(payload)
    await _validate_catalog(db, payload)
    await acquire_market_slice_locks(
        db,
        [
            (
                "ASK",
                payload.product_id,
                payload.delivery_point_id,
                payload.availability_window,
            )
        ],
    )
    await _lock_publisher(db, current_user)
    await _validate_catalog(db, payload)
    offer = SupplierOffer(
        supplier_org_id=current_user.organization_id,
        supplier_user_id=current_user.id,
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
        quantity_mt=payload.quantity_mt,
        min_fill_mt=payload.min_fill_mt,
        price_per_mt_usd=payload.price_per_mt_usd,
        availability_window=payload.availability_window,
        listing_terms=listing_terms_payload(payload),
        expires_at=payload.expires_at,
        idempotency_key=key,
        idempotency_request_hash=digest,
    )
    db.add(offer)
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=SUPPLIER_OFFER_CREATED,
        resource_type="supplier_offer",
        resource_id=offer.id,
        changes={
            "offer": offer_snapshot(offer).model_dump(mode="json"),
            "status": offer.status,
        },
        **request_audit_context(request),
    )
    await _commit_event(db, offer, "supplier_offer_created")
    return offer_response(offer, current_user, viewer_is_eligible=True)


@router.put("/{offer_id}", response_model=SupplierOfferResponse)
@limiter.limit("30/minute")
@retry_market_transaction()
async def update_offer(
    request: Request,
    offer_id: UUID,
    payload: SupplierOfferUpdate,
    db: Database,
    current_user: Publisher,
    _admission: Admission,
):
    validate_offer_dates(payload)
    await _validate_catalog(db, payload)
    offer = await _load_owned_locked(db, offer_id, current_user, next_payload=payload)
    if offer.revision != payload.expected_revision:
        raise HTTPException(
            status_code=409,
            detail="Supplier offer changed; reload before making changes",
        )
    if (
        offer.product_id != payload.product_id
        or offer.delivery_point_id != payload.delivery_point_id
    ):
        raise HTTPException(
            status_code=422, detail="Product and delivery point cannot change"
        )
    await _lock_publisher(db, current_user)
    previous = offer_snapshot(offer).model_dump(mode="json")
    previous_status = offer.status
    offer.quantity_mt = payload.quantity_mt
    offer.min_fill_mt = payload.min_fill_mt
    offer.price_per_mt_usd = payload.price_per_mt_usd
    offer.availability_window = payload.availability_window
    offer.listing_terms = listing_terms_payload(payload)
    offer.expires_at = payload.expires_at
    offer.status = "OPEN"
    offer.revision += 1
    offer.updated_at = datetime.now(UTC)
    await record_audit(
        db,
        user_id=current_user.id,
        action=SUPPLIER_OFFER_UPDATED,
        resource_type="supplier_offer",
        resource_id=offer.id,
        changes={
            "from": previous,
            "to": offer_snapshot(offer).model_dump(mode="json"),
            "status": {"from": previous_status, "to": offer.status},
        },
        **request_audit_context(request),
    )
    await _commit_event(db, offer, "supplier_offer_updated")
    return offer_response(offer, current_user, viewer_is_eligible=True)


@router.post("/{offer_id}/withdraw", response_model=SupplierOfferResponse)
@limiter.limit("30/minute")
@retry_market_transaction()
async def withdraw_offer(
    request: Request,
    offer_id: UUID,
    payload: SupplierOfferWithdraw,
    db: Database,
    current_user: Reader,
):
    offer = await _load_owned_locked(db, offer_id, current_user)
    await _lock_publisher(db, current_user, retract=True)
    if offer.status == "WITHDRAWN":
        return offer_response(
            offer,
            current_user,
            viewer_is_eligible=await execution_party_is_eligible(db, user=current_user),
        )
    if offer.revision != payload.expected_revision:
        raise HTTPException(
            status_code=409, detail="Supplier offer changed; reload before withdrawing"
        )
    previous = offer_snapshot(offer).model_dump(mode="json")
    offer.status = "WITHDRAWN"
    offer.revision += 1
    offer.updated_at = datetime.now(UTC)
    await record_audit(
        db,
        user_id=current_user.id,
        action=SUPPLIER_OFFER_WITHDRAWN,
        resource_type="supplier_offer",
        resource_id=offer.id,
        changes={
            "from": previous,
            "to": offer_snapshot(offer).model_dump(mode="json"),
            "status": {"from": "OPEN", "to": "WITHDRAWN"},
        },
        **request_audit_context(request),
    )
    await _commit_event(db, offer, "supplier_offer_withdrawn")
    return offer_response(
        offer,
        current_user,
        viewer_is_eligible=await execution_party_is_eligible(db, user=current_user),
    )

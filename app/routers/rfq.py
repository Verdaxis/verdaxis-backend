"""RFQ (Request for Quote) router — bilateral negotiation alongside the orderbook."""
import uuid
from datetime import datetime, timedelta, UTC
from typing import Annotated, Optional
from zoneinfo import ZoneInfo

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy import and_, or_, select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.rate_limit import limiter
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.models.user import User, UserRole, Organization
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus
from app.models.catalog import Product, DeliveryPoint
from app.models.notification import NotificationType
from app.schemas.rfq import (
    RFQCreateRequest,
    RFQQuoteRequest,
    RFQQuoteRevisionRequest,
    RFQQuoteResponse,
    RFQResponse,
    RFQListResponse,
)
from app.schemas.fame import FameContractTerms
from app.services.fame_rfq import delivery_deadline, quote_compatibility_errors, quote_expiry
from app.services.availability_windows import (
    is_tradable_availability_window,
    normalize_availability_window,
)
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    RFQ_CANCELLED,
    RFQ_CREATED,
    RFQ_QUOTE_SUBMITTED,
    RFQ_QUOTE_REVISED,
    RFQ_QUOTE_WITHDRAWN,
)
from app.services.market_admission import (
    MarketActorOwnership,
    assert_market_pair_provenance,
    assert_market_participant_provenance,
    lock_and_load_market_organizations,
)
from app.services.market_catalog_validation import require_canonical_market_slice
from app.services.market_locks import acquire_market_slice_lock
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.security_market_admission import require_security_market_admission
from app.services.market_transactions import retry_market_transaction
from app.services.org_notifications import notify_org_users as _notify_org_users

router = APIRouter(prefix="/rfq", tags=["rfq"])

_SUPPLIER_VISIBLE_STATUSES = (RFQStatus.OPEN, RFQStatus.QUOTED)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def _load_rfq(
    db: AsyncSession,
    rfq_id: uuid.UUID,
    *,
    with_quotes: bool = False,
    for_update: bool = False,
    visibility_filters: tuple = (),
) -> RFQ:
    """Load an RFQ by ID, optionally with quotes eager-loaded."""
    stmt = select(RFQ).where(RFQ.id == rfq_id, *visibility_filters)
    if with_quotes:
        stmt = stmt.options(selectinload(RFQ.quotes))
    if for_update:
        stmt = stmt.with_for_update().execution_options(populate_existing=True)
    result = await db.execute(stmt)
    rfq = result.unique().scalar_one_or_none()
    if rfq is None:
        raise HTTPException(status_code=404, detail="RFQ not found")
    return rfq


def _rfq_lock_identity(rfq: RFQ) -> tuple[object, ...]:
    return (
        rfq.buyer_org_id,
        rfq.product_id,
        rfq.delivery_point_id,
        str(rfq.availability_window),
    )


def _validate_quote_payload(rfq: RFQ, payload: RFQQuoteRequest) -> datetime:
    deadline = rfq.expires_at
    if rfq.contract_terms is not None:
        if payload.offer_terms is None:
            raise HTTPException(status_code=422, detail="offer_terms are required for UCOME B100")
        contract = FameContractTerms.model_validate(rfq.contract_terms)
        errors = quote_compatibility_errors(contract, payload.offer_terms, rfq.quantity_mt)
        if errors:
            raise HTTPException(status_code=422, detail="; ".join(errors))
        deadline = min(deadline, delivery_deadline(contract.delivery_end))
    elif payload.offer_terms is not None:
        raise HTTPException(status_code=422, detail="FAME offer terms require a structured UCOME B100 RFQ")
    try:
        return quote_expiry(
            payload.expires_at,
            deadline,
            structured=rfq.contract_terms is not None,
        )
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


def _quote_snapshot(quote: RFQQuote) -> dict:
    """Preserve each commercial version in the append-only audit trail."""
    return {
        "revision": quote.revision,
        "price_per_mt_usd": str(quote.price_per_mt_usd),
        "notes": quote.notes,
        "offer_terms": quote.offer_terms,
        "expires_at": quote.expires_at.isoformat() if quote.expires_at else None,
        "status": quote.status.value,
    }


def _owned_quote(rfq: RFQ, quote_id: uuid.UUID, current_user: User) -> RFQQuote:
    quote = next((item for item in rfq.quotes if item.id == quote_id), None)
    if quote is None or quote.seller_org_id != current_user.organization_id:
        raise HTTPException(status_code=404, detail="Quote not found")
    return quote


async def _quote_response(db: AsyncSession, rfq: RFQ, quote: RFQQuote) -> RFQQuoteResponse:
    result = await db.execute(select(Organization.name).where(Organization.id == quote.seller_org_id))
    expires_at = quote.expires_at or rfq.expires_at
    return RFQQuoteResponse(
        id=quote.id,
        seller_org_id=quote.seller_org_id,
        seller_org_name=result.scalar_one_or_none(),
        price_per_mt_usd=quote.price_per_mt_usd,
        notes=quote.notes,
        status=quote.status.value,
        created_at=quote.created_at,
        expires_at=expires_at,
        is_expired=expires_at <= datetime.now(UTC),
        revision=quote.revision,
        offer_terms=quote.offer_terms,
    )


def _rfq_visibility_filters(current_user: User, *, now: datetime | None = None) -> tuple:
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")
    if current_user.role == UserRole.SUPPLIER:
        return (
            RFQ.buyer_org_id != current_user.organization_id,
            or_(
                and_(
                    RFQ.status.in_(_SUPPLIER_VISIBLE_STATUSES),
                    RFQ.expires_at > (now or datetime.now(UTC)),
                ),
                RFQ.quotes.any(RFQQuote.seller_org_id == current_user.organization_id),
            ),
        )
    return (RFQ.buyer_org_id == current_user.organization_id,)


def _ensure_rfq_detail_visible(rfq: RFQ, current_user: User, *, now: datetime | None = None) -> None:
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")
    visible = rfq.buyer_org_id == current_user.organization_id
    if current_user.role == UserRole.SUPPLIER:
        visible = (
            rfq.buyer_org_id != current_user.organization_id
            and (
                (rfq.status in _SUPPLIER_VISIBLE_STATUSES and rfq.expires_at > (now or datetime.now(UTC)))
                or any(q.seller_org_id == current_user.organization_id for q in getattr(rfq, "quotes", []))
            )
        )
    if not visible:
        raise HTTPException(status_code=404, detail="RFQ not found")


async def _lock_rfq_for_quote_change(
    db: AsyncSession,
    rfq_id: uuid.UUID,
    current_user: User,
    *,
    require_quotable: bool = True,
) -> RFQ:
    """Serialize quotes and cancellation, then revalidate current actor ownership."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    if require_quotable and current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can submit quotes")

    visibility_filters = (
        _rfq_visibility_filters(current_user)
        if require_quotable
        else (RFQ.quotes.any(RFQQuote.seller_org_id == current_user.organization_id),)
    )
    observed_rfq = await _load_rfq(db, rfq_id, visibility_filters=visibility_filters)
    observed_identity = _rfq_lock_identity(observed_rfq)
    await acquire_market_slice_lock(
        db,
        side="BID",
        product_id=observed_rfq.product_id,
        delivery_point_id=observed_rfq.delivery_point_id,
        availability_window=str(observed_rfq.availability_window),
    )
    rfq = await _load_rfq(
        db,
        rfq_id,
        with_quotes=True,
        for_update=True,
        visibility_filters=visibility_filters,
    )
    if _rfq_lock_identity(rfq) != observed_identity:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail="RFQ market slice changed; retry the request",
        )
    # Quote rows are market rows too. Lock the current set before user/org
    # revalidation so cancel/quote paths share one global order.
    await db.execute(
        select(RFQQuote)
        .where(RFQQuote.rfq_id == rfq.id)
        .order_by(RFQQuote.id)
        .with_for_update()
    )
    organizations = await lock_and_load_market_organizations(
        db,
        [rfq.buyer_org_id, current_user.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
        require_approved=require_quotable,
        require_execution_eligible=require_quotable,
    )
    if not require_quotable:
        return rfq
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can submit quotes")

    assert_market_pair_provenance(
        organizations[rfq.buyer_org_id],
        organizations[current_user.organization_id],
    )
    await require_canonical_market_slice(
        db,
        product_id=rfq.product_id,
        delivery_point_id=rfq.delivery_point_id,
    )

    # Validate RFQ is quotable
    if rfq.status not in (RFQStatus.OPEN, RFQStatus.QUOTED):
        raise HTTPException(status_code=400, detail="RFQ is not open for quoting")

    if rfq.expires_at <= datetime.now(UTC):
        raise HTTPException(status_code=400, detail="RFQ has expired")

    # Cannot quote own RFQ
    if rfq.buyer_org_id == current_user.organization_id:
        raise HTTPException(status_code=400, detail="Cannot quote your own RFQ")

    return rfq


async def _build_rfq_response(
    db: AsyncSession,
    rfq: RFQ,
    *,
    viewer_org_id: uuid.UUID | None = None,
    viewer_user_id: uuid.UUID | None = None,
) -> RFQResponse:
    """Build an RFQResponse with denormalized names and quote filtering."""
    # Fetch org name
    buyer_org_name = None
    result = await db.execute(select(Organization.name).where(Organization.id == rfq.buyer_org_id))
    row = result.scalar_one_or_none()
    if row:
        buyer_org_name = row

    is_owner = viewer_org_id == rfq.buyer_org_id if viewer_org_id else False
    # Anonymous RFQs never expose a stable buyer tenant identifier to other
    # organizations. Owners retain their normal management view.
    if rfq.is_anonymous and not is_owner:
        buyer_org_name = "Anonymous"

    # Product name
    product_name = None
    result = await db.execute(select(Product.name).where(Product.id == rfq.product_id))
    row = result.scalar_one_or_none()
    if row:
        product_name = row

    # Delivery point name
    delivery_point_name = None
    if rfq.delivery_point_id:
        result = await db.execute(select(DeliveryPoint.name).where(DeliveryPoint.id == rfq.delivery_point_id))
        row = result.scalar_one_or_none()
        if row:
            delivery_point_name = row

    # Build quotes list — filter for suppliers (they see only their own quotes)
    quotes: list[RFQQuoteResponse] = []
    if hasattr(rfq, "quotes") and rfq.quotes:
        for q in rfq.quotes:
            if is_owner or (viewer_org_id and q.seller_org_id == viewer_org_id):
                quotes.append(await _quote_response(db, rfq, q))

    return RFQResponse(
        id=rfq.id,
        buyer_org_id=rfq.buyer_org_id if is_owner or not rfq.is_anonymous else None,
        buyer_org_name=buyer_org_name,
        product_id=rfq.product_id,
        product_name=product_name,
        delivery_point_id=rfq.delivery_point_id,
        delivery_point_name=delivery_point_name,
        quantity_mt=rfq.quantity_mt,
        target_price_per_mt=rfq.target_price_per_mt,
        availability_window=normalize_availability_window(str(rfq.availability_window)),
        notes=rfq.notes,
        is_anonymous=rfq.is_anonymous,
        status=rfq.status.value,
        expires_at=rfq.expires_at,
        created_at=rfq.created_at,
        quote_count=len(rfq.quotes) if hasattr(rfq, "quotes") and rfq.quotes else 0,
        quotes=quotes,
        contract_terms=getattr(rfq, "contract_terms", None),
        can_cancel=is_owner and rfq.status in _SUPPLIER_VISIBLE_STATUSES and (
            getattr(rfq, "buyer_user_id", None) == viewer_user_id
            if getattr(rfq, "buyer_user_id", None) is not None
            else is_owner
        ),
    )


# ---------------------------------------------------------------------------
# 1. POST /rfq — Create RFQ (buyer only)
# ---------------------------------------------------------------------------

@router.post("", response_model=RFQResponse, status_code=status.HTTP_201_CREATED)
@limiter.limit("30/minute")
@retry_market_transaction()
async def create_rfq(
    request: Request,
    payload: RFQCreateRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[
        None,
        Depends(require_security_market_admission),
    ],
):
    """Create a new Request for Quote. Only buyers can create RFQs."""
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="User must belong to an organization",
        )

    if current_user.role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can create RFQs",
        )

    availability_window = normalize_availability_window(payload.availability_window)
    if not is_tradable_availability_window(availability_window):
        raise HTTPException(status_code=400, detail="Availability window is not tradable")
    await require_canonical_market_slice(
        db,
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
    )
    await acquire_market_slice_lock(
        db,
        side="BID",
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
        availability_window=availability_window,
    )
    organizations = await lock_and_load_market_organizations(
        db,
        [current_user.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )
    assert_market_participant_provenance(
        organizations[current_user.organization_id]
    )
    if current_user.role != UserRole.BUYER:
        raise HTTPException(status_code=403, detail="Only buyers can create RFQs")
    product, delivery_point = await require_canonical_market_slice(
        db,
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
    )
    expires_at = datetime.now(UTC) + timedelta(hours=payload.expires_in_hours)
    if product.market_product == "UCOME_B100":
        if payload.contract_terms is None:
            raise HTTPException(status_code=422, detail="contract_terms are required for UCOME B100")
        if delivery_point.name != "Singapore":
            raise HTTPException(status_code=422, detail="UCOME B100 pilot delivery is limited to Singapore")
        if payload.contract_terms.delivery_start < datetime.now(ZoneInfo("Asia/Singapore")).date():
            raise HTTPException(status_code=422, detail="Delivery start must not be in the past")
        if payload.quantity_mt < product.min_lot_size:
            raise HTTPException(status_code=422, detail=f"UCOME B100 RFQ quantity must be at least {product.min_lot_size} MT")
        if expires_at > delivery_deadline(payload.contract_terms.delivery_end):
            raise HTTPException(status_code=422, detail="RFQ deadline must be on or before the delivery end date in Singapore")
    elif payload.contract_terms is not None:
        raise HTTPException(status_code=422, detail="FAME contract terms require the UCOME B100 product")

    rfq = RFQ(
        buyer_org_id=current_user.organization_id,
        buyer_user_id=current_user.id,
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
        quantity_mt=payload.quantity_mt,
        target_price_per_mt=payload.target_price_per_mt,
        contract_terms=payload.contract_terms.model_dump(mode="json") if payload.contract_terms else None,
        notes=payload.notes,
        is_anonymous=payload.is_anonymous,
        expires_at=expires_at,
        quotes=[],
    )

    rfq.availability_window = availability_window

    db.add(rfq)
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=RFQ_CREATED,
        resource_type="rfq",
        resource_id=rfq.id,
        changes={
            "buyer_org_id": str(rfq.buyer_org_id),
            "product_id": str(rfq.product_id),
            "delivery_point_id": str(rfq.delivery_point_id) if rfq.delivery_point_id else None,
            "quantity_mt": str(rfq.quantity_mt),
            "target_price_per_mt": str(rfq.target_price_per_mt) if rfq.target_price_per_mt else None,
            "availability_window": rfq.availability_window,
            "status": rfq.status.value,
            "contract_terms": rfq.contract_terms,
        },
        **request_audit_context(request),
    )

    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="rfq_created",
                aggregate_type="rfq",
                aggregate_id=rfq.id,
                participant_org_ids=(rfq.buyer_org_id,),
                payload={
                    "rfq_id": str(rfq.id),
                    "product_id": str(rfq.product_id),
                    "delivery_point_id": str(rfq.delivery_point_id),
                    "availability_window": rfq.availability_window,
                },
            )
        ],
    )
    await db.commit()

    return await _build_rfq_response(db, rfq, viewer_org_id=current_user.organization_id, viewer_user_id=current_user.id)


# ---------------------------------------------------------------------------
# 2. GET /rfq — List RFQs
# ---------------------------------------------------------------------------

@router.get("", response_model=RFQListResponse)
async def list_rfqs(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    status_filter: Optional[str] = Query(None, alias="status"),
    product_id: Optional[uuid.UUID] = Query(None),
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
):
    """List RFQs. Buyers see their own; suppliers see OPEN RFQs from other orgs."""
    org_id = current_user.organization_id
    filters = list(_rfq_visibility_filters(current_user))

    if status_filter:
        try:
            filters.append(RFQ.status == RFQStatus(status_filter))
        except ValueError:
            pass  # Ignore invalid status filter

    if product_id:
        filters.append(RFQ.product_id == product_id)

    # Count
    count_stmt = select(func.count(RFQ.id)).where(*filters)
    total = (await db.execute(count_stmt)).scalar() or 0

    # Data
    stmt = (
        select(RFQ)
        .options(selectinload(RFQ.quotes))
        .where(*filters)
        .order_by(RFQ.created_at.desc())
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    rfqs = result.unique().scalars().all()

    items = []
    for rfq in rfqs:
        items.append(await _build_rfq_response(db, rfq, viewer_org_id=org_id, viewer_user_id=current_user.id))

    return RFQListResponse(items=items, total=total)


# ---------------------------------------------------------------------------
# 3. GET /rfq/{rfq_id} — Get RFQ detail
# ---------------------------------------------------------------------------

@router.get("/{rfq_id}", response_model=RFQResponse)
async def get_rfq(
    rfq_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Get RFQ detail with quotes. Buyers see all quotes; suppliers see only their own."""
    visibility_filters = _rfq_visibility_filters(current_user)
    rfq = await _load_rfq(
        db,
        rfq_id,
        with_quotes=True,
        visibility_filters=visibility_filters,
    )
    _ensure_rfq_detail_visible(rfq, current_user)

    org_id = current_user.organization_id
    return await _build_rfq_response(db, rfq, viewer_org_id=org_id, viewer_user_id=current_user.id)


# ---------------------------------------------------------------------------
# 4. POST /rfq/{rfq_id}/quote — Submit quote (supplier only)
# ---------------------------------------------------------------------------

@router.post("/{rfq_id}/quote", response_model=RFQQuoteResponse, status_code=status.HTTP_201_CREATED)
@limiter.limit("30/minute")
@retry_market_transaction()
async def submit_quote(
    request: Request,
    rfq_id: uuid.UUID,
    payload: RFQQuoteRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[
        None,
        Depends(require_security_market_admission),
    ],
):
    """Submit a quote on an RFQ. Only suppliers can quote."""
    rfq = await _lock_rfq_for_quote_change(db, rfq_id, current_user)
    expires_at = _validate_quote_payload(rfq, payload)

    # One quote per supplier per RFQ
    existing = [q for q in rfq.quotes if q.seller_org_id == current_user.organization_id]
    if existing:
        raise HTTPException(status_code=409, detail="You have already quoted this RFQ")

    previous_status = rfq.status
    quote = RFQQuote(
        rfq_id=rfq.id,
        seller_org_id=current_user.organization_id,
        seller_user_id=current_user.id,
        price_per_mt_usd=payload.price_per_mt_usd,
        notes=payload.notes,
        expires_at=expires_at,
        offer_terms=payload.offer_terms.model_dump(mode="json") if payload.offer_terms else None,
        revision=1,
    )
    db.add(quote)

    # Update RFQ status to QUOTED if first quote
    if rfq.status == RFQStatus.OPEN:
        rfq.status = RFQStatus.QUOTED

    await db.flush()

    # Notify buyer
    await _notify_org_users(
        db,
        rfq.buyer_org_id,
        NotificationType.ORDER_UPDATE,
        "New Quote Received",
        f"A supplier submitted a quote of ${payload.price_per_mt_usd}/MT on your RFQ.",
        {"rfq_id": str(rfq.id), "quote_id": str(quote.id)},
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=RFQ_QUOTE_SUBMITTED,
        resource_type="rfq_quote",
        resource_id=quote.id,
        changes={
            "rfq_id": str(rfq.id),
            "seller_org_id": str(quote.seller_org_id),
            "price_per_mt_usd": str(quote.price_per_mt_usd),
            "quote": _quote_snapshot(quote),
            "quote_status": quote.status.value,
            "rfq_status": {"from": previous_status.value, "to": rfq.status.value},
        },
        **request_audit_context(request),
    )

    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="rfq_quote_submitted",
                aggregate_type="rfq_quote",
                aggregate_id=quote.id,
                participant_org_ids=(rfq.buyer_org_id, quote.seller_org_id),
                payload={
                    "rfq_id": str(rfq.id),
                    "quote_id": str(quote.id),
                    "status": quote.status.value,
                },
            )
        ],
    )
    await db.commit()

    return await _quote_response(db, rfq, quote)


@router.put("/{rfq_id}/quotes/{quote_id}", response_model=RFQQuoteResponse)
@limiter.limit("30/minute")
@retry_market_transaction()
async def revise_quote(
    request: Request,
    rfq_id: uuid.UUID,
    quote_id: uuid.UUID,
    payload: RFQQuoteRevisionRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[None, Depends(require_security_market_admission)],
):
    """Replace a supplier's quote, retaining the previous version in audit."""
    rfq = await _lock_rfq_for_quote_change(db, rfq_id, current_user)
    quote = _owned_quote(rfq, quote_id, current_user)
    if payload.expected_revision != quote.revision:
        raise HTTPException(status_code=409, detail="Quote was revised; reload it before making changes")
    if quote.status not in (QuoteStatus.PENDING, QuoteStatus.WITHDRAWN):
        raise HTTPException(status_code=409, detail="Quote cannot be revised in its current state")
    expires_at = _validate_quote_payload(rfq, payload)
    previous = _quote_snapshot(quote)
    quote.price_per_mt_usd = payload.price_per_mt_usd
    quote.notes = payload.notes
    quote.offer_terms = payload.offer_terms.model_dump(mode="json") if payload.offer_terms else None
    quote.expires_at = expires_at
    quote.revision += 1
    quote.status = QuoteStatus.PENDING
    await record_audit(
        db,
        user_id=current_user.id,
        action=RFQ_QUOTE_REVISED,
        resource_type="rfq_quote",
        resource_id=quote.id,
        changes={"rfq_id": str(rfq.id), "from": previous, "to": _quote_snapshot(quote)},
        **request_audit_context(request),
    )
    await _notify_org_users(
        db, rfq.buyer_org_id, NotificationType.ORDER_UPDATE,
        "Quote Revised", "A supplier revised a quote on your RFQ.",
        {"rfq_id": str(rfq.id), "quote_id": str(quote.id)},
    )
    await enqueue_market_events(db, [participant_market_event(
        event_type="rfq_quote_revised",
        aggregate_type="rfq_quote",
        aggregate_id=quote.id,
        participant_org_ids=(rfq.buyer_org_id, quote.seller_org_id),
        payload={"rfq_id": str(rfq.id), "quote_id": str(quote.id), "revision": quote.revision},
    )])
    await db.commit()
    return await _quote_response(db, rfq, quote)


@router.post("/{rfq_id}/quotes/{quote_id}/withdraw", response_model=RFQQuoteResponse)
@limiter.limit("30/minute")
@retry_market_transaction()
async def withdraw_quote(
    request: Request,
    rfq_id: uuid.UUID,
    quote_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_authenticated_user)],
):
    """Allow an organization to retract its quote after admission or expiry changes."""
    rfq = await _lock_rfq_for_quote_change(db, rfq_id, current_user, require_quotable=False)
    quote = _owned_quote(rfq, quote_id, current_user)
    if quote.status == QuoteStatus.WITHDRAWN:
        return await _quote_response(db, rfq, quote)
    if quote.status != QuoteStatus.PENDING:
        raise HTTPException(status_code=409, detail="Quote cannot be withdrawn in its current state")
    previous = _quote_snapshot(quote)
    # A form opened before withdrawal must reload before it can reopen the quote.
    quote.revision += 1
    quote.status = QuoteStatus.WITHDRAWN
    await record_audit(
        db,
        user_id=current_user.id,
        action=RFQ_QUOTE_WITHDRAWN,
        resource_type="rfq_quote",
        resource_id=quote.id,
        changes={"rfq_id": str(rfq.id), "from": previous, "to": _quote_snapshot(quote)},
        **request_audit_context(request),
    )
    await _notify_org_users(
        db, rfq.buyer_org_id, NotificationType.ORDER_UPDATE,
        "Quote Withdrawn", "A supplier withdrew a quote on your RFQ.",
        {"rfq_id": str(rfq.id), "quote_id": str(quote.id)},
    )
    await enqueue_market_events(db, [participant_market_event(
        event_type="rfq_quote_withdrawn",
        aggregate_type="rfq_quote",
        aggregate_id=quote.id,
        participant_org_ids=(rfq.buyer_org_id, quote.seller_org_id),
        payload={"rfq_id": str(rfq.id), "quote_id": str(quote.id), "status": quote.status.value},
    )])
    await db.commit()
    return await _quote_response(db, rfq, quote)


# ---------------------------------------------------------------------------
# 5. POST /rfq/{rfq_id}/accept/{quote_id} — Accept quote (buyer only)
# ---------------------------------------------------------------------------

@router.post("/{rfq_id}/accept/{quote_id}", response_model=RFQQuoteResponse)
@limiter.limit("30/minute")
async def accept_quote(
    request: Request,
    rfq_id: uuid.UUID,
    quote_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[
        None,
        Depends(require_security_market_admission),
    ],
):
    """Execution is disabled until a separately reviewed bilateral contract ships."""
    raise HTTPException(
        status_code=409,
        detail="RFQ acceptance is disabled; RFQs are non-executable in this release",
    )


# ---------------------------------------------------------------------------
# 6. POST /rfq/{rfq_id}/cancel — Cancel RFQ (buyer only)
# ---------------------------------------------------------------------------

@router.post("/{rfq_id}/cancel")
@limiter.limit("30/minute")
@retry_market_transaction()
async def cancel_rfq(
    request: Request,
    rfq_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_authenticated_user)],
):
    """Cancel an owned RFQ even after its owner's execution eligibility changes."""
    observed_rfq = await _load_rfq(db, rfq_id)
    observed_identity = _rfq_lock_identity(observed_rfq)
    await acquire_market_slice_lock(
        db,
        side="BID",
        product_id=observed_rfq.product_id,
        delivery_point_id=observed_rfq.delivery_point_id,
        availability_window=str(observed_rfq.availability_window),
    )
    rfq = await _load_rfq(db, rfq_id, with_quotes=True, for_update=True)
    if _rfq_lock_identity(rfq) != observed_identity:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail="RFQ market slice changed; retry the request",
        )
    await db.execute(
        select(RFQQuote)
        .where(RFQQuote.rfq_id == rfq.id)
        .order_by(RFQQuote.id)
        .with_for_update()
    )
    await lock_and_load_market_organizations(
        db,
        [rfq.buyer_org_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
        require_approved=False,
        require_execution_eligible=False,
    )

    # Must own the RFQ
    owns_rfq = current_user.organization_id == rfq.buyer_org_id and (
        rfq.buyer_user_id is None or rfq.buyer_user_id == current_user.id
    )
    if not owns_rfq:
        raise HTTPException(status_code=403, detail="Only the RFQ owner can cancel")

    if rfq.status not in (RFQStatus.OPEN, RFQStatus.QUOTED):
        raise HTTPException(status_code=400, detail="RFQ cannot be cancelled in its current state")

    previous_status = rfq.status
    rfq.status = RFQStatus.CANCELLED

    # Withdraw all pending quotes
    if rfq.quotes:
        for q in rfq.quotes:
            if q.status == QuoteStatus.PENDING:
                q.status = QuoteStatus.WITHDRAWN
                await record_audit(
                    db,
                    user_id=current_user.id,
                    action=RFQ_QUOTE_WITHDRAWN,
                    resource_type="rfq_quote",
                    resource_id=q.id,
                    changes={
                        "rfq_id": str(rfq.id),
                        "quote_status": {
                            "from": QuoteStatus.PENDING.value,
                            "to": QuoteStatus.WITHDRAWN.value,
                        },
                    },
                    **request_audit_context(request),
                )

    await record_audit(
        db,
        user_id=current_user.id,
        action=RFQ_CANCELLED,
        resource_type="rfq",
        resource_id=rfq.id,
        changes={"status": {"from": previous_status.value, "to": RFQStatus.CANCELLED.value}},
        **request_audit_context(request),
    )
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="rfq_cancelled",
                aggregate_type="rfq",
                aggregate_id=rfq.id,
                participant_org_ids={
                    rfq.buyer_org_id,
                    *(quote.seller_org_id for quote in rfq.quotes),
                },
                payload={"rfq_id": str(rfq.id), "status": rfq.status.value},
            )
        ],
    )
    await db.commit()

    return {"detail": "RFQ cancelled successfully"}

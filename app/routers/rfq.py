"""RFQ (Request for Quote) router — bilateral negotiation alongside the orderbook."""
import uuid
from datetime import datetime, timedelta, UTC
from typing import Annotated, Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.rate_limit import limiter
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.middleware.execution import require_execution_eligible_user
from app.models.user import User, UserRole, Organization
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus
from app.models.catalog import Product, DeliveryPoint
from app.models.notification import Notification, NotificationType
from app.models.orderbook import Trade, TradeStatus, Initiator
from app.schemas.rfq import (
    RFQCreateRequest,
    RFQQuoteRequest,
    RFQQuoteResponse,
    RFQResponse,
    RFQListResponse,
)
from app.services.event_bus import event_bus
from app.services.availability_windows import normalize_availability_window
from app.services.activity import publish_trade_event, trade_activity_provenance
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    RFQ_ACCEPTED,
    RFQ_CANCELLED,
    RFQ_CREATED,
    RFQ_QUOTE_SUBMITTED,
    TRADE_CREATED,
)
from app.services.behavioral_analytics import track_analytics_event, trade_created_event
from app.services.execution_policy import execution_party_is_eligible

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
        stmt = stmt.with_for_update()
    result = await db.execute(stmt)
    rfq = result.unique().scalar_one_or_none()
    if rfq is None:
        raise HTTPException(status_code=404, detail="RFQ not found")
    return rfq


def _rfq_visibility_filters(current_user: User, *, now: datetime | None = None) -> tuple:
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")
    if current_user.role == UserRole.SUPPLIER:
        return (
            RFQ.buyer_org_id != current_user.organization_id,
            RFQ.status.in_(_SUPPLIER_VISIBLE_STATUSES),
            RFQ.expires_at > (now or datetime.now(UTC)),
        )
    return (RFQ.buyer_org_id == current_user.organization_id,)


def _ensure_rfq_detail_visible(rfq: RFQ, current_user: User, *, now: datetime | None = None) -> None:
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")
    visible = rfq.buyer_org_id == current_user.organization_id
    if current_user.role == UserRole.SUPPLIER:
        visible = (
            rfq.buyer_org_id != current_user.organization_id
            and rfq.status in _SUPPLIER_VISIBLE_STATUSES
            and rfq.expires_at > (now or datetime.now(UTC))
        )
    if not visible:
        raise HTTPException(status_code=404, detail="RFQ not found")


async def _build_rfq_response(db: AsyncSession, rfq: RFQ, *, viewer_org_id: uuid.UUID | None = None) -> RFQResponse:
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
                # Fetch seller org name
                seller_org_name = None
                res = await db.execute(select(Organization.name).where(Organization.id == q.seller_org_id))
                sn = res.scalar_one_or_none()
                if sn:
                    seller_org_name = sn
                quotes.append(RFQQuoteResponse(
                    id=q.id,
                    seller_org_id=q.seller_org_id,
                    seller_org_name=seller_org_name,
                    price_per_mt_usd=q.price_per_mt_usd,
                    notes=q.notes,
                    status=q.status.value,
                    created_at=q.created_at,
                ))

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
    )


async def _notify_org_users(
    db: AsyncSession,
    org_id: uuid.UUID,
    notif_type: NotificationType,
    title: str,
    message: str,
    data: dict | None = None,
):
    """Send a notification to every user in the given organization."""
    stmt = select(User).where(User.organization_id == org_id)
    result = await db.execute(stmt)
    users = result.scalars().all()
    for user in users:
        db.add(
            Notification(
                recipient_id=user.id,
                type=notif_type,
                title=title,
                message=message,
                data=data or {},
            )
        )


# ---------------------------------------------------------------------------
# 1. POST /rfq — Create RFQ (buyer only)
# ---------------------------------------------------------------------------

@router.post("", response_model=RFQResponse, status_code=status.HTTP_201_CREATED)
@limiter.limit("30/minute")
async def create_rfq(
    request: Request,
    payload: RFQCreateRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
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

    # Validate product exists
    product_result = await db.execute(select(Product).where(Product.id == payload.product_id))
    if not product_result.scalars().first():
        raise HTTPException(status_code=400, detail="Invalid product_id")

    # Validate delivery point if provided
    if payload.delivery_point_id:
        dp_result = await db.execute(select(DeliveryPoint).where(DeliveryPoint.id == payload.delivery_point_id))
        if not dp_result.scalars().first():
            raise HTTPException(status_code=400, detail="Invalid delivery_point_id")

    rfq = RFQ(
        buyer_org_id=current_user.organization_id,
        buyer_user_id=current_user.id,
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
        quantity_mt=payload.quantity_mt,
        target_price_per_mt=payload.target_price_per_mt,
        notes=payload.notes,
        is_anonymous=payload.is_anonymous,
        expires_at=datetime.now(UTC) + timedelta(hours=payload.expires_in_hours),
    )

    rfq.availability_window = normalize_availability_window(payload.availability_window)

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
        },
        **request_audit_context(request),
    )

    # Eagerly set quotes to empty list for response building
    rfq.quotes = []

    await db.commit()

    return await _build_rfq_response(db, rfq, viewer_org_id=current_user.organization_id)


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
    if not org_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    filters = []

    if current_user.role == UserRole.SUPPLIER:
        # Suppliers see OPEN/QUOTED RFQs that are not their own and not expired
        filters.append(RFQ.buyer_org_id != org_id)
        filters.append(RFQ.status.in_([RFQStatus.OPEN, RFQStatus.QUOTED]))
        filters.append(RFQ.expires_at > datetime.now(UTC))
    else:
        # Buyers see their own RFQs (all statuses)
        filters.append(RFQ.buyer_org_id == org_id)

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
        items.append(await _build_rfq_response(db, rfq, viewer_org_id=org_id))

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
    return await _build_rfq_response(db, rfq, viewer_org_id=org_id)


# ---------------------------------------------------------------------------
# 4. POST /rfq/{rfq_id}/quote — Submit quote (supplier only)
# ---------------------------------------------------------------------------

@router.post("/{rfq_id}/quote", response_model=RFQQuoteResponse, status_code=status.HTTP_201_CREATED)
@limiter.limit("30/minute")
async def submit_quote(
    request: Request,
    rfq_id: uuid.UUID,
    payload: RFQQuoteRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Submit a quote on an RFQ. Only suppliers can quote."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can submit quotes")

    rfq = await _load_rfq(
        db,
        rfq_id,
        with_quotes=True,
        for_update=True,
        visibility_filters=_rfq_visibility_filters(current_user),
    )

    # Validate RFQ is quotable
    if rfq.status not in (RFQStatus.OPEN, RFQStatus.QUOTED):
        raise HTTPException(status_code=400, detail="RFQ is not open for quoting")

    if rfq.expires_at <= datetime.now(UTC):
        raise HTTPException(status_code=400, detail="RFQ has expired")

    # Cannot quote own RFQ
    if rfq.buyer_org_id == current_user.organization_id:
        raise HTTPException(status_code=400, detail="Cannot quote your own RFQ")

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
            "quote_status": quote.status.value,
            "rfq_status": {"from": previous_status.value, "to": rfq.status.value},
        },
        **request_audit_context(request),
    )

    await db.commit()

    # Fetch seller org name for response
    seller_org_name = None
    res = await db.execute(select(Organization.name).where(Organization.id == current_user.organization_id))
    sn = res.scalar_one_or_none()
    if sn:
        seller_org_name = sn

    return RFQQuoteResponse(
        id=quote.id,
        seller_org_id=quote.seller_org_id,
        seller_org_name=seller_org_name,
        price_per_mt_usd=quote.price_per_mt_usd,
        notes=quote.notes,
        status=quote.status.value,
        created_at=quote.created_at,
    )


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
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Accept a quote on an RFQ. Creates a trade and declines all other quotes."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    rfq = await _load_rfq(
        db,
        rfq_id,
        with_quotes=True,
        for_update=True,
        visibility_filters=_rfq_visibility_filters(current_user),
    )

    # Must own the RFQ
    if rfq.buyer_org_id != current_user.organization_id:
        raise HTTPException(status_code=403, detail="Only the RFQ owner can accept quotes")
    if rfq.buyer_user_id != current_user.id:
        raise HTTPException(status_code=403, detail="Only the initiating user can accept quotes")

    # RFQ must be open or quoted
    if rfq.status not in (RFQStatus.OPEN, RFQStatus.QUOTED):
        raise HTTPException(status_code=400, detail="RFQ is not in a quotable state")

    # Lock and load the target quote separately (prevents race condition)
    quote_stmt = (
        select(RFQQuote)
        .where(RFQQuote.id == quote_id, RFQQuote.rfq_id == rfq_id)
        .with_for_update()
    )
    quote_result = await db.execute(quote_stmt)
    target_quote = quote_result.scalar_one_or_none()

    if target_quote is None:
        raise HTTPException(status_code=404, detail="Quote not found")

    if target_quote.status != QuoteStatus.PENDING:
        raise HTTPException(status_code=400, detail="Quote is not pending")

    # Revalidate the RFQ owner and selected quote owner while both market
    # records are locked. A stale eligible caller cannot execute a revoked
    # counterparty's quote.
    if not rfq.buyer_user_id or not target_quote.seller_user_id:
        raise HTTPException(status_code=409, detail="RFQ parties require fresh admission review")
    party_result = await db.execute(
        select(User)
        .where(User.id.in_([rfq.buyer_user_id, target_quote.seller_user_id]))
        .with_for_update()
    )
    parties = {party.id: party for party in party_result.scalars().all()}
    org_result = await db.execute(
        select(Organization)
        .where(Organization.id.in_([rfq.buyer_org_id, target_quote.seller_org_id]))
        .with_for_update()
    )
    organizations = {org.id: org for org in org_result.scalars().all()}
    if not await execution_party_is_eligible(
        db, user=parties.get(rfq.buyer_user_id), organization=organizations.get(rfq.buyer_org_id)
    ) or not await execution_party_is_eligible(
        db, user=parties.get(target_quote.seller_user_id), organization=organizations.get(target_quote.seller_org_id)
    ):
        raise HTTPException(status_code=409, detail="RFQ parties are no longer execution-qualified")

    # Accept the target quote, decline all others
    previous_rfq_status = rfq.status
    previous_quote_status = target_quote.status
    target_quote.status = QuoteStatus.ACCEPTED
    for q in rfq.quotes:
        if q.id != quote_id and q.status == QuoteStatus.PENDING:
            q.status = QuoteStatus.DECLINED

    rfq.status = RFQStatus.ACCEPTED

    # Create a Trade (following the trades.py pattern)
    trade = Trade(
        bid_order_id=None,
        ask_order_id=None,
        buyer_id=rfq.buyer_org_id,
        seller_id=target_quote.seller_org_id,
        buyer_user_id=rfq.buyer_user_id,
        seller_user_id=target_quote.seller_user_id,
        initiated_by=Initiator.BUYER,
        quantity_mt=rfq.quantity_mt,
        price_per_mt_usd=target_quote.price_per_mt_usd,
        status=TradeStatus.CONFIRMED,
        confirmed_at=datetime.now(UTC),
    )
    db.add(trade)

    await db.flush()

    # Notify seller
    await _notify_org_users(
        db,
        target_quote.seller_org_id,
        NotificationType.ORDER_UPDATE,
        "Quote Accepted",
        f"Your quote of ${target_quote.price_per_mt_usd}/MT has been accepted!",
        {"rfq_id": str(rfq.id), "trade_id": str(trade.id)},
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=RFQ_ACCEPTED,
        resource_type="rfq",
        resource_id=rfq.id,
        changes={
            "rfq_status": {"from": previous_rfq_status.value, "to": RFQStatus.ACCEPTED.value},
            "quote_status": {"from": previous_quote_status.value, "to": QuoteStatus.ACCEPTED.value},
            "quote_id": str(target_quote.id),
            "seller_org_id": str(target_quote.seller_org_id),
            "trade_id": str(trade.id),
            "price_per_mt_usd": str(target_quote.price_per_mt_usd),
        },
        **request_audit_context(request),
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_CREATED,
        resource_type="trade",
        resource_id=trade.id,
        changes={
            "via": "rfq",
            "rfq_id": str(rfq.id),
            "quote_id": str(target_quote.id),
            "quantity_mt": str(trade.quantity_mt),
            "price_per_mt_usd": str(trade.price_per_mt_usd),
            "buyer_org_id": str(trade.buyer_id),
            "seller_org_id": str(trade.seller_id),
        },
        **request_audit_context(request),
    )

    await db.commit()
    track_analytics_event(
        trade_created_event(
            current_user,
            availability_window=rfq.availability_window,
            request=request,
        ),
        request=request,
    )

    # Emit SSE event
    await publish_trade_event(trade, "trade_created", {
        **trade_activity_provenance(trade),
        "id": str(trade.id),
        "status": trade.status.value,
        "quantity": str(trade.quantity_mt),
        "price": str(trade.price_per_mt_usd),
        "source": "rfq",
    })

    # Fetch seller org name for response
    seller_org_name = None
    res = await db.execute(select(Organization.name).where(Organization.id == target_quote.seller_org_id))
    sn = res.scalar_one_or_none()
    if sn:
        seller_org_name = sn

    return RFQQuoteResponse(
        id=target_quote.id,
        seller_org_id=target_quote.seller_org_id,
        seller_org_name=seller_org_name,
        price_per_mt_usd=target_quote.price_per_mt_usd,
        notes=target_quote.notes,
        status=target_quote.status.value,
        created_at=target_quote.created_at,
    )


# ---------------------------------------------------------------------------
# 6. POST /rfq/{rfq_id}/cancel — Cancel RFQ (buyer only)
# ---------------------------------------------------------------------------

@router.post("/{rfq_id}/cancel")
@limiter.limit("30/minute")
async def cancel_rfq(
    request: Request,
    rfq_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_authenticated_user)],
):
    """Cancel an owned RFQ even after its owner's execution eligibility changes."""
    rfq = await _load_rfq(
        db,
        rfq_id,
        with_quotes=True,
        for_update=True,
    )

    # Must own the RFQ
    owns_rfq = (
        rfq.buyer_user_id == current_user.id
        if rfq.buyer_user_id is not None
        else current_user.organization_id is not None
        and rfq.buyer_org_id == current_user.organization_id
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
        action=RFQ_CANCELLED,
        resource_type="rfq",
        resource_id=rfq.id,
        changes={"status": {"from": previous_status.value, "to": RFQStatus.CANCELLED.value}},
        **request_audit_context(request),
    )
    await db.commit()

    return {"detail": "RFQ cancelled successfully"}

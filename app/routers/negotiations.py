"""Negotiations router — buyer/seller counteroffer flow."""
import uuid
from datetime import datetime, timedelta, UTC
from typing import Annotated, Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy import select, func, or_, and_
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.rate_limit import limiter
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.middleware.execution import require_execution_eligible_user
from app.models.user import User, UserRole, Organization
from app.models.negotiation import Negotiation, NegotiationRound, NegotiationStatus
from app.models.catalog import Product
from app.models.orderbook import (
    Initiator,
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.models.notification import Notification, NotificationType
from app.schemas.negotiation import (
    NegotiationCreateRequest,
    NegotiationCounterRequest,
    NegotiationResponse,
    NegotiationRoundResponse,
    NegotiationListResponse,
)
from app.services.activity import publish_trade_event, trade_activity_provenance
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    NEGOTIATION_ACCEPTED,
    NEGOTIATION_COUNTERED,
    NEGOTIATION_CREATED,
    NEGOTIATION_DECLINED,
    TRADE_CREATED,
)
from app.services.event_bus import event_bus
from app.services.behavioral_analytics import track_analytics_event, trade_created_event
from app.services.execution_policy import execution_party_is_eligible, order_is_execution_qualified

router = APIRouter(prefix="/negotiations", tags=["negotiations"])

MAX_ROUNDS = 10  # Cap to prevent infinite back-and-forth


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

async def _load_negotiation(
    db: AsyncSession,
    negotiation_id: uuid.UUID,
    org_id: uuid.UUID,
    *,
    with_rounds: bool = False,
    for_update: bool = False,
) -> Negotiation:
    """Load a negotiation, filtering to party membership to avoid 403/404 oracle."""
    stmt = select(Negotiation).where(
        Negotiation.id == negotiation_id,
        or_(
            Negotiation.initiator_org_id == org_id,
            Negotiation.counterparty_org_id == org_id,
        ),
    )
    if with_rounds:
        stmt = stmt.options(selectinload(Negotiation.rounds))
    if for_update:
        stmt = stmt.with_for_update(nowait=True)
    result = await db.execute(stmt)
    neg = result.unique().scalar_one_or_none()
    if neg is None:
        # Return 404 regardless of whether the negotiation exists — prevents oracle
        raise HTTPException(status_code=404, detail="Negotiation not found")
    return neg


async def _batch_org_names(db: AsyncSession, org_ids: set[uuid.UUID]) -> dict[uuid.UUID, str]:
    """Fetch org names for a set of IDs in a single query."""
    if not org_ids:
        return {}
    result = await db.execute(
        select(Organization.id, Organization.name).where(Organization.id.in_(org_ids))
    )
    return {row.id: row.name for row in result}


async def _build_response(db: AsyncSession, neg: Negotiation) -> NegotiationResponse:
    # Collect all org IDs we need in one batch
    org_ids: set[uuid.UUID] = {neg.initiator_org_id, neg.counterparty_org_id}
    rounds_list = neg.rounds if hasattr(neg, "rounds") and neg.rounds else []
    for r in rounds_list:
        org_ids.add(r.proposer_org_id)

    org_names = await _batch_org_names(db, org_ids)

    product_name = None
    result = await db.execute(select(Product.name).where(Product.id == neg.product_id))
    product_name = result.scalar_one_or_none()

    rounds: list[NegotiationRoundResponse] = [
        NegotiationRoundResponse(
            id=r.id,
            round_number=r.round_number,
            proposer_org_id=r.proposer_org_id,
            proposer_org_name=org_names.get(r.proposer_org_id),
            proposer_user_id=r.proposer_user_id,
            proposed_price=r.proposed_price,
            notes=r.notes,
            created_at=r.created_at,
        )
        for r in rounds_list
    ]

    return NegotiationResponse(
        id=neg.id,
        bid_order_id=neg.bid_order_id,
        ask_order_id=neg.ask_order_id,
        initiator_org_id=neg.initiator_org_id,
        initiator_org_name=org_names.get(neg.initiator_org_id),
        counterparty_org_id=neg.counterparty_org_id,
        counterparty_org_name=org_names.get(neg.counterparty_org_id),
        initiator_user_id=neg.initiator_user_id,
        counterparty_user_id=neg.counterparty_user_id,
        accepted_by_user_id=neg.accepted_by_user_id,
        initiator_side=neg.initiator_side,
        product_id=neg.product_id,
        product_name=product_name,
        quantity_mt=neg.quantity_mt,
        current_price=neg.current_price,
        status=neg.status.value,
        last_actor_org_id=neg.last_actor_org_id,
        trade_id=neg.trade_id,
        expires_at=neg.expires_at,
        created_at=neg.created_at,
        updated_at=neg.updated_at,
        rounds=rounds,
    )


async def _notify_org_users(
    db: AsyncSession,
    org_id: uuid.UUID,
    notif_type: NotificationType,
    title: str,
    message: str,
    data: dict | None = None,
):
    stmt = select(User).where(User.organization_id == org_id)
    result = await db.execute(stmt)
    for user in result.scalars().all():
        db.add(Notification(
            recipient_id=user.id,
            type=notif_type,
            title=title,
            message=message,
            data=data or {},
        ))


def _assert_active(neg: Negotiation) -> None:
    """Raise 400 if the negotiation is not in an actionable state, persisting EXPIRED if needed."""
    if neg.status not in (NegotiationStatus.OPEN, NegotiationStatus.COUNTERED):
        raise HTTPException(
            status_code=400,
            detail=f"Negotiation is {neg.status.value} — no further actions allowed",
        )
    if neg.expires_at <= datetime.now(UTC):
        # Persist the expired state so future reads reflect reality
        neg.status = NegotiationStatus.EXPIRED
        raise HTTPException(status_code=400, detail="Negotiation has expired")


def _assert_counterparty_turn(neg: Negotiation, org_id: uuid.UUID) -> None:
    if neg.last_actor_org_id == org_id:
        raise HTTPException(
            status_code=400,
            detail="Waiting for the other party to respond",
        )


def _resolve_trade_roles(neg: Negotiation) -> tuple[uuid.UUID, uuid.UUID]:
    """Return (buyer_org_id, seller_org_id) from initiator_side.
    Call only after the negotiation's order references have been validated.
    """
    if neg.initiator_side == "BUYER":
        return neg.initiator_org_id, neg.counterparty_org_id
    return neg.counterparty_org_id, neg.initiator_org_id


def _resolve_trade_users(neg: Negotiation) -> tuple[uuid.UUID, uuid.UUID]:
    """Return concrete (buyer_user_id, seller_user_id), rejecting legacy ambiguity."""
    if not neg.initiator_user_id or not neg.counterparty_user_id:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "NEGOTIATION_PROVENANCE_REQUIRED",
                "message": "Legacy negotiation requires an explicit audited provenance decision",
            },
        )
    if neg.initiator_side == "BUYER":
        return neg.initiator_user_id, neg.counterparty_user_id
    if neg.initiator_side == "SELLER":
        return neg.counterparty_user_id, neg.initiator_user_id
    raise HTTPException(status_code=409, detail="Negotiation party roles are invalid")


def _assert_concrete_party(neg: Negotiation, current_user: User) -> None:
    expected = {
        neg.initiator_user_id: neg.initiator_org_id,
        neg.counterparty_user_id: neg.counterparty_org_id,
    }
    expected_org = expected.get(current_user.id)
    if expected_org is None or current_user.organization_id != expected_org:
        raise HTTPException(status_code=404, detail="Negotiation not found")


async def _revalidate_negotiation_parties(
    db: AsyncSession,
    neg: Negotiation,
) -> tuple[dict[uuid.UUID, User], dict[uuid.UUID, Organization]]:
    """Lock and revalidate both concrete user/organization pairs in this transaction."""
    buyer_user_id, seller_user_id = _resolve_trade_users(neg)
    users = {
        user.id: user
        for user in (
            await db.execute(
                select(User)
                .where(User.id.in_([buyer_user_id, seller_user_id]))
                .order_by(User.id)
                .with_for_update(nowait=True)
            )
        ).scalars().all()
    }
    buyer_org_id, seller_org_id = _resolve_trade_roles(neg)
    organizations = {
        organization.id: organization
        for organization in (
            await db.execute(
                select(Organization)
                .where(Organization.id.in_([buyer_org_id, seller_org_id]))
                .order_by(Organization.id)
                .with_for_update(nowait=True)
            )
        ).scalars().all()
    }
    buyer = users.get(buyer_user_id)
    seller = users.get(seller_user_id)
    if (
        getattr(buyer, "role", None) != UserRole.BUYER
        or getattr(seller, "role", None) != UserRole.SUPPLIER
        or not await execution_party_is_eligible(
            db, user=buyer, organization=organizations.get(buyer_org_id)
        )
        or not await execution_party_is_eligible(
            db, user=seller, organization=organizations.get(seller_org_id)
        )
    ):
        raise HTTPException(
            status_code=409,
            detail={
                "code": "NEGOTIATION_PARTY_INELIGIBLE",
                "message": "One or more negotiation parties are no longer execution-qualified",
            },
        )
    return users, organizations


async def _load_negotiation_orders(
    db: AsyncSession,
    neg: Negotiation,
    *,
    require_available: bool,
) -> dict[uuid.UUID, OrderBookOrder]:
    order_ids = sorted(
        (order_id for order_id in (neg.bid_order_id, neg.ask_order_id) if order_id),
        key=str,
    )
    orders = {
        order.id: order
        for order in (
            await db.execute(
                select(OrderBookOrder)
                .where(OrderBookOrder.id.in_(order_ids))
                .order_by(OrderBookOrder.id)
                .with_for_update(nowait=True)
            )
        ).scalars().all()
    }
    buyer_user_id, seller_user_id = _resolve_trade_users(neg)
    buyer_org_id, seller_org_id = _resolve_trade_roles(neg)
    expected = (
        (neg.bid_order_id, OrderSide.BID, buyer_org_id, buyer_user_id),
        (neg.ask_order_id, OrderSide.ASK, seller_org_id, seller_user_id),
    )
    for order_id, side, organization_id, owner_user_id in expected:
        if order_id is None:
            continue
        order = orders.get(order_id)
        if (
            order is None
            or order.side != side
            or order.organization_id != organization_id
            or order.owner_user_id != owner_user_id
            or order.product_id != neg.product_id
        ):
            raise HTTPException(status_code=409, detail="Negotiation order provenance changed")
        if require_available and (
            order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
            or (order.expires_at is not None and order.expires_at <= datetime.now(UTC))
            or not order_is_execution_qualified(order)
            or order.remaining_quantity_mt < neg.quantity_mt
        ):
            raise HTTPException(status_code=409, detail="Negotiation order is no longer executable")
    return orders


def _consume_locked_order_capacity(
    neg: Negotiation,
    orders: dict[uuid.UUID, OrderBookOrder],
) -> None:
    """Consume unfilled capacity after canonical rows have been locked/validated."""
    for order_id in (neg.bid_order_id, neg.ask_order_id):
        if order_id is None:
            continue
        order = orders[order_id]
        order.remaining_quantity_mt -= neg.quantity_mt
        if order.remaining_quantity_mt <= 0:
            order.remaining_quantity_mt = 0
            order.status = OrderBookStatus.FILLED
        else:
            order.status = OrderBookStatus.PARTIALLY_FILLED


def _is_lock_unavailable(exc: DBAPIError) -> bool:
    original = getattr(exc, "orig", None)
    return getattr(original, "sqlstate", None) in {"55P03", "57014"}


def _capacity_busy_error() -> HTTPException:
    return HTTPException(
        status_code=409,
        detail={
            "code": "NEGOTIATION_CAPACITY_BUSY",
            "message": "Order capacity is being updated; retry the negotiation",
        },
    )


async def _consume_negotiation_capacity(
    db: AsyncSession,
    neg: Negotiation,
) -> dict[uuid.UUID, OrderBookOrder]:
    try:
        orders = await _load_negotiation_orders(db, neg, require_available=True)
    except DBAPIError as exc:
        if _is_lock_unavailable(exc):
            raise _capacity_busy_error() from exc
        raise
    _consume_locked_order_capacity(neg, orders)
    return orders


async def _load_owned_negotiation_for_decline(
    db: AsyncSession,
    negotiation_id: uuid.UUID,
    user_id: uuid.UUID,
) -> Negotiation:
    result = await db.execute(
        select(Negotiation)
        .options(selectinload(Negotiation.rounds))
        .where(
            Negotiation.id == negotiation_id,
            or_(
                Negotiation.initiator_user_id == user_id,
                Negotiation.counterparty_user_id == user_id,
            ),
        )
        .with_for_update()
    )
    neg = result.unique().scalar_one_or_none()
    if neg is None:
        raise HTTPException(status_code=404, detail="Negotiation not found")
    return neg


# ---------------------------------------------------------------------------
# 1. POST /negotiations — Initiate negotiation
# ---------------------------------------------------------------------------

@router.post("", response_model=NegotiationResponse, status_code=status.HTTP_201_CREATED)
@limiter.limit("30/minute")
async def create_negotiation(
    request: Request,
    payload: NegotiationCreateRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Initiate a price negotiation with another organization."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id

    if payload.counterparty_org_id == org_id:
        raise HTTPException(status_code=400, detail="Cannot negotiate with yourself")

    if current_user.role == UserRole.BUYER:
        initiator_side = "BUYER"
        if payload.ask_order_id is None:
            raise HTTPException(status_code=400, detail="Buyer negotiations require a counterparty ASK order")
    elif current_user.role == UserRole.SUPPLIER:
        initiator_side = "SELLER"
        if payload.bid_order_id is None:
            raise HTTPException(status_code=400, detail="Supplier negotiations require a counterparty BID order")
    else:
        raise HTTPException(status_code=403, detail="Administrators cannot initiate negotiations")

    # Validate product
    product_result = await db.execute(select(Product).where(Product.id == payload.product_id))
    if not product_result.scalars().first():
        raise HTTPException(status_code=400, detail="Invalid product_id")

    order_ids = [order_id for order_id in (payload.bid_order_id, payload.ask_order_id) if order_id]
    orders = {
        order.id: order
        for order in (
            await db.execute(
                select(OrderBookOrder).where(OrderBookOrder.id.in_(order_ids)).with_for_update()
            )
        ).scalars().all()
    }
    bid_order = orders.get(payload.bid_order_id) if payload.bid_order_id else None
    ask_order = orders.get(payload.ask_order_id) if payload.ask_order_id else None
    expected_orders = (
        (payload.bid_order_id, bid_order, OrderSide.BID),
        (payload.ask_order_id, ask_order, OrderSide.ASK),
    )
    for order_id, order, side in expected_orders:
        if order_id is None:
            continue
        if order is None or order.side != side:
            raise HTTPException(status_code=400, detail=f"Invalid {side.value.lower()}_order_id")
        if (
            order.product_id != payload.product_id
            or order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
            or (order.expires_at is not None and order.expires_at <= datetime.now(UTC))
            or not order_is_execution_qualified(order)
            or order.remaining_quantity_mt < payload.quantity_mt
        ):
            raise HTTPException(status_code=409, detail="Referenced order is not execution-qualified")

    if initiator_side == "BUYER":
        if ask_order.organization_id != payload.counterparty_org_id or not ask_order.owner_user_id:
            raise HTTPException(status_code=400, detail="ASK order does not identify the declared counterparty")
        if bid_order and (
            bid_order.organization_id != org_id or bid_order.owner_user_id != current_user.id
        ):
            raise HTTPException(status_code=400, detail="BID order must be owned by the initiating user")
        counterparty_user_id = ask_order.owner_user_id
        expected_counterparty_role = UserRole.SUPPLIER
    else:
        if bid_order.organization_id != payload.counterparty_org_id or not bid_order.owner_user_id:
            raise HTTPException(status_code=400, detail="BID order does not identify the declared counterparty")
        if ask_order and (
            ask_order.organization_id != org_id or ask_order.owner_user_id != current_user.id
        ):
            raise HTTPException(status_code=400, detail="ASK order must be owned by the initiating user")
        counterparty_user_id = bid_order.owner_user_id
        expected_counterparty_role = UserRole.BUYER

    locked_users = {
        user.id: user
        for user in (
            await db.execute(
                select(User)
                .where(User.id.in_([current_user.id, counterparty_user_id]))
                .with_for_update()
            )
        ).scalars().all()
    }
    organizations = {
        organization.id: organization
        for organization in (
            await db.execute(
                select(Organization)
                .where(Organization.id.in_([org_id, payload.counterparty_org_id]))
                .with_for_update()
            )
        ).scalars().all()
    }
    locked_initiator = locked_users.get(current_user.id)
    counterparty_user = locked_users.get(counterparty_user_id)
    if (
        getattr(locked_initiator, "role", None) != current_user.role
        or getattr(counterparty_user, "role", None) != expected_counterparty_role
        or not await execution_party_is_eligible(
            db, user=locked_initiator, organization=organizations.get(org_id)
        )
        or not await execution_party_is_eligible(
            db,
            user=counterparty_user,
            organization=organizations.get(payload.counterparty_org_id),
        )
    ):
        raise HTTPException(status_code=409, detail="Negotiation party is not execution-qualified")

    # Reject duplicate active negotiation between the same two parties for the same product
    duplicate = await db.execute(
        select(Negotiation).where(
            and_(
                or_(
                    and_(
                        Negotiation.initiator_org_id == org_id,
                        Negotiation.counterparty_org_id == payload.counterparty_org_id,
                    ),
                    and_(
                        Negotiation.initiator_org_id == payload.counterparty_org_id,
                        Negotiation.counterparty_org_id == org_id,
                    ),
                ),
                Negotiation.product_id == payload.product_id,
                Negotiation.status.in_(["OPEN", "COUNTERED"]),
            )
        )
    )
    if duplicate.scalar_one_or_none():
        raise HTTPException(
            status_code=409,
            detail="An active negotiation already exists for this product with this counterparty",
        )

    neg = Negotiation(
        bid_order_id=payload.bid_order_id,
        ask_order_id=payload.ask_order_id,
        initiator_org_id=org_id,
        counterparty_org_id=payload.counterparty_org_id,
        initiator_user_id=current_user.id,
        counterparty_user_id=counterparty_user_id,
        initiator_side=initiator_side,
        product_id=payload.product_id,
        quantity_mt=payload.quantity_mt,
        current_price=payload.proposed_price,
        last_actor_org_id=org_id,
        expires_at=datetime.now(UTC) + timedelta(hours=payload.expires_in_hours),
    )
    db.add(neg)
    await db.flush()

    round1 = NegotiationRound(
        negotiation_id=neg.id,
        round_number=1,
        proposer_org_id=org_id,
        proposer_user_id=current_user.id,
        proposed_price=payload.proposed_price,
        notes=payload.notes,
    )
    db.add(round1)
    await record_audit(
        db,
        user_id=current_user.id,
        action=NEGOTIATION_CREATED,
        resource_type="negotiation",
        resource_id=neg.id,
        changes={
            "initiator_org_id": str(neg.initiator_org_id),
            "counterparty_org_id": str(neg.counterparty_org_id),
            "initiator_user_id": str(neg.initiator_user_id),
            "counterparty_user_id": str(neg.counterparty_user_id),
            "product_id": str(neg.product_id),
            "bid_order_id": str(neg.bid_order_id) if neg.bid_order_id else None,
            "ask_order_id": str(neg.ask_order_id) if neg.ask_order_id else None,
            "quantity_mt": str(neg.quantity_mt),
            "price_per_mt_usd": str(neg.current_price),
            "status": neg.status.value,
        },
        **request_audit_context(request),
    )

    org_names = await _batch_org_names(db, {org_id})
    proposer_name = org_names.get(org_id)
    await _notify_org_users(
        db,
        payload.counterparty_org_id,
        NotificationType.NEGOTIATION_RECEIVED,
        "New Deal Proposal",
        f"{proposer_name or 'A counterparty'} proposed ${payload.proposed_price}/MT for {payload.quantity_mt} MT.",
        {"negotiation_id": str(neg.id)},
    )

    await db.commit()

    await event_bus.publish("negotiation", "negotiation_created", {
        "id": str(neg.id),
        "counterparty_org_id": str(payload.counterparty_org_id),
        "proposed_price": str(payload.proposed_price),
    })

    neg.rounds = [round1]
    return await _build_response(db, neg)


# ---------------------------------------------------------------------------
# 2. GET /negotiations — List my negotiations
# ---------------------------------------------------------------------------

@router.get("", response_model=NegotiationListResponse)
async def list_negotiations(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    status_filter: Optional[str] = Query(None, alias="status"),
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
):
    """List negotiations I'm party to (as initiator or counterparty)."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id
    now = datetime.now(UTC)

    # Only return logically active or terminal negotiations — exclude expired-but-unsettled
    filters = [
        or_(
            Negotiation.initiator_org_id == org_id,
            Negotiation.counterparty_org_id == org_id,
        ),
        # Exclude negotiations that timed out but whose status was never persisted as EXPIRED
        or_(
            Negotiation.status.not_in(["OPEN", "COUNTERED"]),
            Negotiation.expires_at > now,
        ),
    ]

    if status_filter:
        try:
            filters.append(Negotiation.status == NegotiationStatus(status_filter))
        except ValueError:
            pass

    count_stmt = select(func.count(Negotiation.id)).where(*filters)
    total = (await db.execute(count_stmt)).scalar() or 0

    stmt = (
        select(Negotiation)
        .options(selectinload(Negotiation.rounds))
        .where(*filters)
        .order_by(Negotiation.created_at.desc())
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    negotiations = result.unique().scalars().all()

    items = [await _build_response(db, neg) for neg in negotiations]
    return NegotiationListResponse(items=items, total=total)


# ---------------------------------------------------------------------------
# 3. GET /negotiations/{id} — Get detail
# ---------------------------------------------------------------------------

@router.get("/{negotiation_id}", response_model=NegotiationResponse)
async def get_negotiation(
    negotiation_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    neg = await _load_negotiation(db, negotiation_id, current_user.organization_id, with_rounds=True)
    return await _build_response(db, neg)


# ---------------------------------------------------------------------------
# 4. POST /negotiations/{id}/counter — Submit counter-offer
# ---------------------------------------------------------------------------

@router.post("/{negotiation_id}/counter", response_model=NegotiationResponse)
@limiter.limit("60/minute")
async def counter_negotiation(
    request: Request,
    negotiation_id: uuid.UUID,
    payload: NegotiationCounterRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Submit a counter-offer price."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id
    neg = await _load_negotiation(db, negotiation_id, org_id, with_rounds=True, for_update=True)

    _assert_concrete_party(neg, current_user)
    await _revalidate_negotiation_parties(db, neg)
    await _load_negotiation_orders(db, neg, require_available=True)
    _assert_active(neg)
    _assert_counterparty_turn(neg, org_id)

    if len(neg.rounds) >= MAX_ROUNDS:
        raise HTTPException(status_code=400, detail=f"Maximum {MAX_ROUNDS} rounds reached")

    previous_price = neg.current_price
    previous_status = neg.status
    next_round_num = len(neg.rounds) + 1
    new_round = NegotiationRound(
        negotiation_id=neg.id,
        round_number=next_round_num,
        proposer_org_id=org_id,
        proposer_user_id=current_user.id,
        proposed_price=payload.proposed_price,
        notes=payload.notes,
    )
    db.add(new_round)

    neg.current_price = payload.proposed_price
    neg.last_actor_org_id = org_id
    neg.status = NegotiationStatus.COUNTERED

    other_org_id = (
        neg.counterparty_org_id if org_id == neg.initiator_org_id else neg.initiator_org_id
    )
    org_names = await _batch_org_names(db, {org_id})
    await _notify_org_users(
        db,
        other_org_id,
        NotificationType.NEGOTIATION_COUNTERED,
        "Counter-Offer Received",
        f"{org_names.get(org_id) or 'Counterparty'} countered at ${payload.proposed_price}/MT.",
        {"negotiation_id": str(neg.id)},
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=NEGOTIATION_COUNTERED,
        resource_type="negotiation",
        resource_id=neg.id,
        changes={
            "status": {"from": previous_status.value, "to": neg.status.value},
            "price_per_mt_usd": {"from": str(previous_price), "to": str(neg.current_price)},
            "round": next_round_num,
            "proposer_org_id": str(org_id),
            "proposer_user_id": str(current_user.id),
        },
        **request_audit_context(request),
    )

    await db.commit()

    await event_bus.publish("negotiation", "negotiation_countered", {
        "id": str(neg.id),
        "proposed_price": str(payload.proposed_price),
        "round": next_round_num,
    })

    neg.rounds.append(new_round)
    return await _build_response(db, neg)


# ---------------------------------------------------------------------------
# 5. POST /negotiations/{id}/accept — Accept current price → create trade
# ---------------------------------------------------------------------------

@router.post("/{negotiation_id}/accept", response_model=NegotiationResponse)
@limiter.limit("30/minute")
async def accept_negotiation(
    request: Request,
    negotiation_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Accept the current proposed price — creates a confirmed trade."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id
    try:
        neg = await _load_negotiation(db, negotiation_id, org_id, with_rounds=True, for_update=True)
        _assert_concrete_party(neg, current_user)
        await _revalidate_negotiation_parties(db, neg)
        _assert_active(neg)
        _assert_counterparty_turn(neg, org_id)
        await _consume_negotiation_capacity(db, neg)
    except DBAPIError as exc:
        if _is_lock_unavailable(exc):
            await db.rollback()
            raise _capacity_busy_error() from exc
        raise
    # Deterministic buyer/seller from initiator_side — not derived from order lookup
    buyer_org, seller_org = _resolve_trade_roles(neg)
    buyer_user_id, seller_user_id = _resolve_trade_users(neg)

    # Safety: caller must be one of the two resolved trade parties
    if org_id not in (buyer_org, seller_org):
        raise HTTPException(status_code=403, detail="Caller is not a party to this trade")

    trade = Trade(
        bid_order_id=neg.bid_order_id,
        ask_order_id=neg.ask_order_id,
        buyer_id=buyer_org,
        seller_id=seller_org,
        buyer_user_id=buyer_user_id,
        seller_user_id=seller_user_id,
        initiated_by=Initiator.BUYER if org_id == buyer_org else Initiator.SELLER,
        quantity_mt=neg.quantity_mt,
        price_per_mt_usd=neg.current_price,
        status=TradeStatus.CONFIRMED,
        confirmed_at=datetime.now(UTC),
    )
    db.add(trade)
    await db.flush()

    previous_status = neg.status
    neg.status = NegotiationStatus.AGREED
    neg.trade_id = trade.id
    neg.accepted_by_user_id = current_user.id

    other_org_id = seller_org if org_id == buyer_org else buyer_org
    org_names = await _batch_org_names(db, {org_id})
    acceptor_name = org_names.get(org_id)

    await _notify_org_users(
        db,
        other_org_id,
        NotificationType.NEGOTIATION_ACCEPTED,
        "Deal Agreed",
        f"{acceptor_name or 'Counterparty'} accepted ${neg.current_price}/MT. Trade confirmed.",
        {"negotiation_id": str(neg.id), "trade_id": str(trade.id)},
    )
    await _notify_org_users(
        db,
        org_id,
        NotificationType.NEGOTIATION_ACCEPTED,
        "Deal Agreed",
        f"You accepted the deal at ${neg.current_price}/MT. Trade confirmed.",
        {"negotiation_id": str(neg.id), "trade_id": str(trade.id)},
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=NEGOTIATION_ACCEPTED,
        resource_type="negotiation",
        resource_id=neg.id,
        changes={
            "status": {"from": previous_status.value, "to": NegotiationStatus.AGREED.value},
            "trade_id": str(trade.id),
            "accepted_by_org_id": str(org_id),
            "accepted_by_user_id": str(current_user.id),
            "price_per_mt_usd": str(neg.current_price),
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
            "via": "negotiation",
            "negotiation_id": str(neg.id),
            "quantity_mt": str(trade.quantity_mt),
            "price_per_mt_usd": str(trade.price_per_mt_usd),
            "buyer_org_id": str(trade.buyer_id),
            "seller_org_id": str(trade.seller_id),
            "buyer_user_id": str(trade.buyer_user_id),
            "seller_user_id": str(trade.seller_user_id),
        },
        **request_audit_context(request),
    )

    await db.commit()
    track_analytics_event(
        trade_created_event(current_user, request=request), request=request
    )

    await event_bus.publish("negotiation", "negotiation_agreed", {
        "id": str(neg.id),
        "trade_id": str(trade.id),
        "price": str(neg.current_price),
    })
    await publish_trade_event(trade, "trade_created", {
        **trade_activity_provenance(trade),
        "id": str(trade.id),
        "status": trade.status.value,
        "quantity": str(trade.quantity_mt),
        "price": str(trade.price_per_mt_usd),
        "source": "negotiation",
    })

    return await _build_response(db, neg)


# ---------------------------------------------------------------------------
# 6. POST /negotiations/{id}/decline — Decline negotiation
# ---------------------------------------------------------------------------

@router.post("/{negotiation_id}/decline", response_model=NegotiationResponse)
@limiter.limit("30/minute")
async def decline_negotiation(
    request: Request,
    negotiation_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_authenticated_user)],
):
    """Decline an owned negotiation even after KYC or organization eligibility changes."""
    neg = await _load_owned_negotiation_for_decline(db, negotiation_id, current_user.id)
    org_id = (
        neg.initiator_org_id
        if neg.initiator_user_id == current_user.id
        else neg.counterparty_org_id
    )

    _assert_active(neg)

    previous_status = neg.status
    neg.status = NegotiationStatus.DECLINED

    other_org_id = (
        neg.counterparty_org_id if org_id == neg.initiator_org_id else neg.initiator_org_id
    )
    org_names = await _batch_org_names(db, {org_id})
    await _notify_org_users(
        db,
        other_org_id,
        NotificationType.NEGOTIATION_DECLINED,
        "Deal Declined",
        f"{org_names.get(org_id) or 'Counterparty'} declined the negotiation.",
        {"negotiation_id": str(neg.id)},
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=NEGOTIATION_DECLINED,
        resource_type="negotiation",
        resource_id=neg.id,
        changes={
            "status": {"from": previous_status.value, "to": NegotiationStatus.DECLINED.value},
            "declined_by_org_id": str(org_id),
        },
        **request_audit_context(request),
    )

    await db.commit()

    await event_bus.publish("negotiation", "negotiation_declined", {
        "id": str(neg.id),
        "declined_by": str(org_id),
    })

    return await _build_response(db, neg)

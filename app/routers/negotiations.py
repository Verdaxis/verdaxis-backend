"""Negotiations router — buyer/seller counteroffer flow."""
import uuid
from datetime import datetime, timedelta, UTC
from typing import Annotated, Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy import select, func, or_, and_
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.rate_limit import limiter
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.models.user import User, UserRole, Organization
from app.models.negotiation import Negotiation, NegotiationRound, NegotiationStatus
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.catalog import Product
from app.models.notification import NotificationType
from app.schemas.negotiation import (
    NegotiationCreateRequest,
    NegotiationCounterRequest,
    NegotiationResponse,
    NegotiationRoundResponse,
    NegotiationListResponse,
)
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    NEGOTIATION_COUNTERED,
    NEGOTIATION_CREATED,
    NEGOTIATION_DECLINED,
)
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.availability_windows import (
    is_tradable_availability_window,
    normalize_availability_window,
)
from app.services.execution_policy import (
    execution_party_is_eligible,
    order_is_execution_qualified,
)
from app.services.market_admission import (
    MarketActorOwnership,
    assert_market_pair_provenance,
    lock_and_load_market_organizations,
)
from app.services.market_catalog_validation import require_canonical_market_slice
from app.services.market_locks import (
    acquire_market_slice_lock,
    acquire_market_slice_locks,
)
from app.services.security_market_admission import require_security_market_admission
from app.services import market_transactions
from app.services.market_transactions import retry_market_transaction
from app.services.org_notifications import notify_org_users as _notify_org_users

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
        stmt = stmt.with_for_update().execution_options(populate_existing=True)
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
        delivery_point_id=neg.delivery_point_id,
        availability_window=neg.availability_window,
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


def _assert_active(neg: Negotiation) -> None:
    """Raise when the non-executable conversation is no longer actionable."""
    if neg.status not in (NegotiationStatus.OPEN, NegotiationStatus.COUNTERED):
        raise HTTPException(
            status_code=400,
            detail=f"Negotiation is {neg.status.value} — no further actions allowed",
        )
    if neg.expires_at <= datetime.now(UTC):
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


def _order_lock_identity(order: OrderBookOrder) -> tuple[object, ...]:
    return (
        order.id,
        order.organization_id,
        order.side,
        order.product_id,
        order.delivery_point_id,
        str(order.availability_window),
    )


def _negotiation_lock_identity(neg: Negotiation) -> tuple[object, ...]:
    return (
        neg.initiator_org_id,
        neg.counterparty_org_id,
        neg.product_id,
        neg.delivery_point_id,
        str(neg.availability_window),
    )


async def _load_order(
    db: AsyncSession,
    order_id: uuid.UUID,
    *,
    for_update: bool,
) -> OrderBookOrder | None:
    stmt = select(OrderBookOrder).where(OrderBookOrder.id == order_id)
    if for_update:
        stmt = stmt.with_for_update().execution_options(populate_existing=True)
    return (await db.execute(stmt)).scalar_one_or_none()


def _validate_negotiation_order(
    order: OrderBookOrder | None,
    *,
    expected_side: OrderSide,
    org_ids: set[uuid.UUID],
    quantity_mt,
) -> OrderBookOrder:
    if order is None:
        raise HTTPException(status_code=400, detail=f"Invalid {expected_side.value.lower()}_order_id")
    if order.side != expected_side:
        raise HTTPException(
            status_code=400,
            detail=f"{expected_side.value.lower()}_order_id must reference a {expected_side.value} order",
        )
    if order.organization_id not in org_ids:
        raise HTTPException(
            status_code=400,
            detail=f"{expected_side.value.lower()}_order_id does not belong to either negotiating party",
        )
    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(
            status_code=409,
            detail=f"{expected_side.value.lower()}_order_id is not executable",
        )
    if order.expires_at is not None and order.expires_at <= datetime.now(UTC):
        raise HTTPException(
            status_code=409,
            detail=f"{expected_side.value.lower()}_order_id has expired",
        )
    if quantity_mt > order.remaining_quantity_mt:
        raise HTTPException(
            status_code=409,
            detail=f"Negotiation quantity exceeds {expected_side.value} capacity",
        )
    if not order_is_execution_qualified(order):
        raise HTTPException(
            status_code=409,
            detail=f"{expected_side.value.lower()}_order_id is not execution-qualified",
        )
    return order


async def _load_and_validate_negotiation_orders(
    db: AsyncSession,
    payload: NegotiationCreateRequest,
    *,
    org_id: uuid.UUID,
    for_update: bool,
) -> tuple[OrderBookOrder | None, OrderBookOrder | None, OrderBookOrder]:
    parties = {org_id, payload.counterparty_org_id}
    bid_order = None
    ask_order = None
    if payload.bid_order_id is not None:
        bid_order = _validate_negotiation_order(
            await _load_order(db, payload.bid_order_id, for_update=for_update),
            expected_side=OrderSide.BID,
            org_ids=parties,
            quantity_mt=payload.quantity_mt,
        )
    if payload.ask_order_id is not None:
        ask_order = _validate_negotiation_order(
            await _load_order(db, payload.ask_order_id, for_update=for_update),
            expected_side=OrderSide.ASK,
            org_ids=parties,
            quantity_mt=payload.quantity_mt,
        )
    canonical_order = bid_order or ask_order
    if canonical_order is None:
        raise HTTPException(status_code=400, detail="A negotiation requires an order")
    if canonical_order.product_id != payload.product_id:
        raise HTTPException(
            status_code=400,
            detail="Negotiation product must match its canonical order",
        )
    expected_window = normalize_availability_window(payload.availability_window)
    if (
        canonical_order.delivery_point_id != payload.delivery_point_id
        or canonical_order.availability_window != expected_window
    ):
        raise HTTPException(
            status_code=400,
            detail="Negotiation market slice must match its canonical order",
        )
    if bid_order is not None and ask_order is not None:
        if (
            bid_order.product_id != ask_order.product_id
            or bid_order.delivery_point_id != ask_order.delivery_point_id
            or bid_order.availability_window != ask_order.availability_window
        ):
            raise HTTPException(
                status_code=400,
                detail="Negotiation orders must share one canonical market slice",
            )
        if {bid_order.organization_id, ask_order.organization_id} != parties:
            raise HTTPException(
                status_code=400,
                detail="Negotiation orders must be owned by opposite negotiating parties",
            )
    elif canonical_order.organization_id != payload.counterparty_org_id:
        raise HTTPException(
            status_code=400,
            detail="A one-sided negotiation must reference the counterparty's order",
        )
    return bid_order, ask_order, canonical_order


async def _lock_negotiation_for_mutation(
    db: AsyncSession,
    negotiation_id: uuid.UUID,
    org_id: uuid.UUID,
    user_id: uuid.UUID,
) -> Negotiation:
    observed = await _load_negotiation(db, negotiation_id, org_id, with_rounds=True)
    observed_identity = _negotiation_lock_identity(observed)
    await acquire_market_slice_lock(
        db,
        side="BID",
        product_id=observed.product_id,
        delivery_point_id=observed.delivery_point_id,
        availability_window=str(observed.availability_window),
    )
    locked = await _load_negotiation(
        db,
        negotiation_id,
        org_id,
        with_rounds=True,
        for_update=True,
    )
    if _negotiation_lock_identity(locked) != observed_identity:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail="Negotiation market slice changed; retry the request",
        )
    await db.execute(
        select(NegotiationRound)
        .where(NegotiationRound.negotiation_id == locked.id)
        .order_by(NegotiationRound.id)
        .with_for_update()
    )
    organizations = await lock_and_load_market_organizations(
        db,
        [locked.initiator_org_id, locked.counterparty_org_id],
        actor_ownerships=(MarketActorOwnership(user_id, org_id),),
    )
    assert_market_pair_provenance(
        organizations[locked.initiator_org_id],
        organizations[locked.counterparty_org_id],
    )
    return locked


# ---------------------------------------------------------------------------
# 1. POST /negotiations — Initiate negotiation
# ---------------------------------------------------------------------------

@router.post("", response_model=NegotiationResponse, status_code=status.HTTP_201_CREATED)
@limiter.limit("30/minute")
@retry_market_transaction()
async def create_negotiation(
    request: Request,
    payload: NegotiationCreateRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[
        None,
        Depends(require_security_market_admission),
    ],
):
    """Initiate a price negotiation with another organization."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id

    if payload.counterparty_org_id == org_id:
        raise HTTPException(status_code=400, detail="Cannot negotiate with yourself")
    if current_user.role not in (UserRole.BUYER, UserRole.SUPPLIER):
        raise HTTPException(status_code=403, detail="User is not a market participant")
    initiator_side = "BUYER" if current_user.role == UserRole.BUYER else "SELLER"
    if initiator_side == "BUYER" and payload.ask_order_id is None:
        raise HTTPException(status_code=400, detail="Buyer negotiations require a counterparty ASK order")
    if initiator_side == "SELLER" and payload.bid_order_id is None:
        raise HTTPException(status_code=400, detail="Supplier negotiations require a counterparty BID order")
    availability_window = normalize_availability_window(payload.availability_window)
    if not is_tradable_availability_window(availability_window):
        raise HTTPException(status_code=400, detail="Availability window is not tradable")

    observed_bid, observed_ask, _observed_canonical = (
        await _load_and_validate_negotiation_orders(
            db,
            payload,
            org_id=org_id,
            for_update=False,
        )
    )
    await require_canonical_market_slice(
        db,
        product_id=payload.product_id,
        delivery_point_id=payload.delivery_point_id,
    )
    observed_orders = [
        order for order in (observed_bid, observed_ask) if order is not None
    ]
    await market_transactions.transaction_boundary_hook(
        "before_market_slice_lock",
        operation="negotiation_create",
        aggregate_id=org_id,
    )
    await acquire_market_slice_locks(
        db,
        [
            (
                order.side,
                order.product_id,
                order.delivery_point_id,
                str(order.availability_window),
            )
            for order in observed_orders
        ],
    )
    bid_order, ask_order, canonical_order = await _load_and_validate_negotiation_orders(
        db,
        payload,
        org_id=org_id,
        for_update=True,
    )
    locked_orders = [order for order in (bid_order, ask_order) if order is not None]
    if [_order_lock_identity(order) for order in locked_orders] != [
        _order_lock_identity(order) for order in observed_orders
    ]:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail="Negotiation order slice changed; retry the request",
        )
    organizations = await lock_and_load_market_organizations(
        db,
        [org_id, payload.counterparty_org_id],
        actor_ownerships=(MarketActorOwnership(current_user.id, org_id),),
    )
    assert_market_pair_provenance(
        organizations[org_id],
        organizations[payload.counterparty_org_id],
    )
    for order in locked_orders:
        organization = organizations[order.organization_id]
        order_provenance = getattr(order.provenance, "value", order.provenance)
        org_provenance = getattr(organization.provenance, "value", organization.provenance)
        if order_provenance != org_provenance:
            await db.rollback()
            raise HTTPException(
                status_code=409,
                detail="Negotiation order provenance is stale",
            )
    await require_canonical_market_slice(
        db,
        product_id=canonical_order.product_id,
        delivery_point_id=canonical_order.delivery_point_id,
    )

    # Concrete party provenance: the counterparty's resting order must carry
    # an owner, and any own-side order must belong to the initiating user.
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

    # Re-read and lock both concrete parties in this transaction; the
    # organizations are already row-locked by lock_and_load_market_organizations.
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
        delivery_point_id=canonical_order.delivery_point_id,
        availability_window=canonical_order.availability_window,
        quantity_mt=payload.quantity_mt,
        current_price=payload.proposed_price,
        last_actor_org_id=org_id,
        expires_at=datetime.now(UTC) + timedelta(hours=payload.expires_in_hours),
    )
    round1 = NegotiationRound(
        round_number=1,
        proposer_org_id=org_id,
        proposer_user_id=current_user.id,
        proposed_price=payload.proposed_price,
        notes=payload.notes,
    )
    # Populate the relationship while both objects are transient. Assigning
    # an unloaded collection after commit would issue implicit async IO and
    # raise MissingGreenlet on PostgreSQL.
    neg.rounds.append(round1)
    db.add(neg)
    try:
        await db.flush()
    except IntegrityError as exc:
        await db.rollback()
        constraint = getattr(getattr(exc.orig, "diag", None), "constraint_name", None)
        if constraint == "uq_negotiations_active_pair_product":
            raise HTTPException(
                status_code=409,
                detail="An active negotiation already exists for this product with this counterparty",
            ) from exc
        raise
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

    response = await _build_response(db, neg)
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="negotiation_created",
                aggregate_type="negotiation",
                aggregate_id=neg.id,
                participant_org_ids=(neg.initiator_org_id, neg.counterparty_org_id),
                payload={
                    "id": str(neg.id),
                    "counterparty_org_id": str(payload.counterparty_org_id),
                    "proposed_price": str(payload.proposed_price),
                },
            )
        ],
    )
    await db.commit()

    return response


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
@retry_market_transaction()
async def counter_negotiation(
    request: Request,
    negotiation_id: uuid.UUID,
    payload: NegotiationCounterRequest,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[
        None,
        Depends(require_security_market_admission),
    ],
):
    """Submit a counter-offer price."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id
    neg = await _lock_negotiation_for_mutation(
        db, negotiation_id, org_id, current_user.id
    )

    _assert_concrete_party(neg, current_user)
    await _revalidate_negotiation_parties(db, neg)
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

    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="negotiation_countered",
                aggregate_type="negotiation",
                aggregate_id=neg.id,
                participant_org_ids=(neg.initiator_org_id, neg.counterparty_org_id),
                payload={
                    "id": str(neg.id),
                    "proposed_price": str(payload.proposed_price),
                    "round": next_round_num,
                },
            )
        ],
    )
    await db.commit()

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
    current_user: Annotated[User, Depends(get_current_user)],
    _security_admission: Annotated[
        None,
        Depends(require_security_market_admission),
    ],
):
    """Execution is disabled until a separately reviewed bilateral contract ships."""
    raise HTTPException(
        status_code=409,
        detail="Negotiation acceptance is disabled; negotiations are non-executable in this release",
    )


# ---------------------------------------------------------------------------
# 6. POST /negotiations/{id}/decline — Decline negotiation
# ---------------------------------------------------------------------------

@router.post("/{negotiation_id}/decline", response_model=NegotiationResponse)
@limiter.limit("30/minute")
@retry_market_transaction()
async def decline_negotiation(
    request: Request,
    negotiation_id: uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_authenticated_user)],
):
    """Decline an owned negotiation even after KYC or organization eligibility changes."""
    if not current_user.organization_id:
        raise HTTPException(status_code=403, detail="User must belong to an organization")

    org_id = current_user.organization_id
    neg = await _lock_negotiation_for_mutation(
        db, negotiation_id, org_id, current_user.id
    )

    # Only a concrete recorded party may decline; legacy rows without user
    # provenance require the audited invalidation path instead.
    if current_user.id not in (neg.initiator_user_id, neg.counterparty_user_id):
        raise HTTPException(status_code=404, detail="Negotiation not found")

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

    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="negotiation_declined",
                aggregate_type="negotiation",
                aggregate_id=neg.id,
                participant_org_ids=(neg.initiator_org_id, neg.counterparty_org_id),
                payload={"id": str(neg.id), "declined_by": str(org_id)},
            )
        ],
    )
    await db.commit()

    return await _build_response(db, neg)

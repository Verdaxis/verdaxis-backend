import uuid
from datetime import datetime, UTC
from decimal import Decimal
from typing import Annotated

from fastapi import APIRouter, Depends, HTTPException, Request, status, Query
from sqlalchemy import select, or_, func
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import joinedload, selectinload
from uuid import UUID

from app.database import get_db
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.middleware.execution import require_execution_eligible_user
from app.models.user import Organization, User, UserRole, OrganizationProvenance
from app.models.orderbook import (
    OrderBookOrder,
    Trade,
    OrderSide,
    OrderBookStatus,
    TradeStatus,
    Initiator,
)
from app.models.notification import NotificationType
from app.schemas.orderbook import TradeCreate, TradeResponse, TradeDeliverPayload
from app.schemas.pagination import PaginatedResponse
from app.services.activity import trade_activity_provenance
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.market_transactions import retry_market_transaction
from app.services import market_transactions
from app.services.watchlist_events import _best_slice_price, emit_order_updated
from app.services.execution_policy import (
    execution_party_is_eligible,
    order_is_execution_qualified,
)
from app.services.live_benchmarks import rebuild_live_slice_benchmarks_for_keys
from app.services.demo_market import is_demo_market_organization
from app.services.provenance import coerce_provenance, execution_provenance_compatible, snapshot_organization_provenance
from app.services.idempotency import (
    TRADE_CREATE_OPERATION,
    acquire_idempotency_lock,
    idempotency_request_hash,
)
from app.services.market_locks import acquire_market_slice_lock
from app.services.market_provenance import trade_market_provenance
from app.services.inventory_reservations import consume_inventory, release_inventory
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    ORDER_EXPIRED,
    TRADE_CONFIRMED,
    TRADE_CREATED,
    TRADE_DECLINED,
    TRADE_DELIVERED,
    TRADE_PAID,
)
from app.schemas.errors import AUTH_RESPONSES
from app.services.behavioral_analytics import track_analytics_event, trade_created_event
from app.services.market_admission import (
    MarketActorOwnership,
    lock_and_load_market_organizations,
)
from app.services.org_notifications import notify_org_users

router = APIRouter(prefix="/trades", tags=["trades"], responses=AUTH_RESPONSES)

# One party reports delivery unilaterally, so the final price it sets must
# stay within this band around the confirmed trade price. Guards commission
# and GMV integrity until a two-sided delivery confirmation flow exists.
MAX_FINAL_PRICE_DEVIATION_PCT = Decimal("10")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def build_trade_response(trade: Trade) -> TradeResponse:
    """Build a response without triggering async lazy loads.

    Market identity is always read from immutable trade snapshots. Party names
    are used only when the caller eagerly loaded those relationships.
    """
    buyer = trade.__dict__.get("buyer")
    seller = trade.__dict__.get("seller")
    buyer_name = buyer.name if buyer is not None else ""
    seller_name = seller.name if seller is not None else ""
    is_anonymous = bool(trade.is_anonymous)
    if is_anonymous and trade.status == TradeStatus.PENDING_CONFIRMATION:
        buyer_name = "Anonymous"
        seller_name = "Anonymous"

    provenance = trade_market_provenance(trade)
    return TradeResponse(
        id=trade.id,
        bid_order_id=trade.bid_order_id,
        ask_order_id=trade.ask_order_id,
        buyer_id=trade.buyer_id,
        seller_id=trade.seller_id,
        buyer_name=buyer_name,
        seller_name=seller_name,
        initiated_by=trade.initiated_by,
        is_anonymous=is_anonymous,
        quantity_mt=trade.quantity_mt,
        price_per_mt_usd=trade.price_per_mt_usd,
        status=trade.status,
        final_quantity_mt=trade.final_quantity_mt,
        final_price_per_mt=trade.final_price_per_mt,
        final_total_usd=trade.final_total_usd,
        commission_rate_pct=trade.commission_rate_pct or Decimal("0.5"),
        commission_amount_usd=trade.commission_amount_usd,
        confirmed_at=trade.confirmed_at,
        delivered_at=trade.delivered_at,
        paid_at=trade.paid_at,
        created_at=trade.created_at,
        product_id=trade.product_id,
        product_name=trade.product_name or "",
        market_product=trade.market_product,
        fuel_type=trade.fuel_type or "",
        fuel_grade=trade.fuel_grade or "",
        delivery_point_id=trade.delivery_point_id,
        delivery_point_name=trade.delivery_point_name,
        region=trade.delivery_point_region or "",
        availability_window=trade.availability_window,
        source_kind=provenance["source_kind"],
        scope=provenance["scope"],
        demo_status=provenance["demo_status"],
        unknown_count=provenance["unknown_count"],
    )


async def _load_trade(db: AsyncSession, trade_id: uuid.UUID, for_update: bool = False) -> Trade:
    """Reload a trade with all relationships needed for the response."""
    stmt = (
        select(Trade)
        .where(Trade.id == trade_id)
        .options(
            selectinload(Trade.buyer),
            selectinload(Trade.seller),
            selectinload(Trade.bid_order).selectinload(OrderBookOrder.product),
            selectinload(Trade.ask_order).selectinload(OrderBookOrder.product),
        )
    )
    if for_update:
        # The same request session may already hold a stale preview identity.
        # Force the locked SELECT to refresh lifecycle fields from PostgreSQL.
        stmt = stmt.with_for_update().execution_options(populate_existing=True)
    result = await db.execute(stmt)
    trade = result.unique().scalar_one_or_none()
    if trade is None:
        raise HTTPException(status_code=404, detail="Trade not found")
    return trade


async def _revalidate_trade_parties(db: AsyncSession, trade: Trade) -> None:
    if not trade.buyer_user_id or not trade.seller_user_id:
        raise HTTPException(status_code=409, detail="Trade parties require fresh admission review")
    # populate_existing: the acting party is already in the session identity
    # map; the locked SELECT must observe a concurrent rejection.
    users_result = await db.execute(
        select(User)
        .where(User.id.in_([trade.buyer_user_id, trade.seller_user_id]))
        .order_by(User.id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    users = {user.id: user for user in users_result.scalars().all()}
    orgs_result = await db.execute(
        select(Organization)
        .where(Organization.id.in_([trade.buyer_id, trade.seller_id]))
        .order_by(Organization.id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    orgs = {org.id: org for org in orgs_result.scalars().all()}
    if not await execution_party_is_eligible(
        db, user=users.get(trade.buyer_user_id), organization=orgs.get(trade.buyer_id)
    ) or not await execution_party_is_eligible(
        db, user=users.get(trade.seller_user_id), organization=orgs.get(trade.seller_id)
    ):
        raise HTTPException(status_code=409, detail="Trade parties are no longer execution-qualified")


def _trade_market_slice(
    trade: Trade,
) -> tuple[OrderSide, UUID, UUID | None, str] | None:
    """Resolve lifecycle locking from an order or immutable trade snapshot."""
    if trade.product_id is not None and trade.availability_window:
        # New trades always carry an immutable snapshot; mutable catalog/order
        # relationships never define a lifecycle lock identity.
        return (
            OrderSide.BID,
            trade.product_id,
            trade.delivery_point_id,
            trade.availability_window,
        )
    order = trade.__dict__.get("ask_order") or trade.__dict__.get("bid_order")
    if order is not None:
        # Nullable legacy snapshots are supported for lifecycle cleanup only.
        return (
            order.side,
            order.product_id,
            order.delivery_point_id,
            order.availability_window,
        )
    return None


async def _lock_trade_market_rows(
    db: AsyncSession,
    trade_id: UUID,
    *,
    operation: str,
) -> tuple[Trade, OrderBookOrder | None]:
    """Lock one trade and linked order under its exact immutable slice."""
    preview = await _load_trade(db, trade_id, for_update=False)
    preview_slice = _trade_market_slice(preview)
    locked_identity = None
    if preview_slice is not None:
        side, product_id, delivery_point_id, availability_window = preview_slice
        locked_identity = (product_id, delivery_point_id, availability_window)
        await market_transactions.transaction_boundary_hook(
            "after_market_preview",
            operation=operation,
            aggregate_id=trade_id,
        )
        await acquire_market_slice_lock(
            db,
            side=side,
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            availability_window=availability_window,
        )

    trade = await _load_trade(db, trade_id, for_update=True)
    current_slice = _trade_market_slice(trade)
    current_identity = (
        (current_slice[1], current_slice[2], current_slice[3])
        if current_slice is not None
        else None
    )
    if current_identity != locked_identity:
        await db.rollback()
        raise HTTPException(status_code=409, detail="Trade slice changed; retry the request")

    order_id = trade.ask_order_id or trade.bid_order_id
    linked_order = None
    if order_id is not None:
        linked_order = (
            await db.execute(
                select(OrderBookOrder)
                .where(OrderBookOrder.id == order_id)
                .with_for_update()
                .execution_options(populate_existing=True)
            )
        ).scalar_one_or_none()
        if linked_order is None:
            await db.rollback()
            raise HTTPException(status_code=409, detail="Trade order no longer exists")
        if locked_identity != (
            linked_order.product_id,
            linked_order.delivery_point_id,
            linked_order.availability_window,
        ):
            await db.rollback()
            raise HTTPException(status_code=409, detail="Trade order slice changed; retry the request")
    return trade, linked_order




async def _watchlist_before_state(db: AsyncSession, order: OrderBookOrder) -> dict[str, object]:
    return {
        "price_per_mt_usd": order.price_per_mt_usd,
        "remaining_quantity_mt": order.remaining_quantity_mt,
        "status": order.status,
        "slice_best_price_per_mt_usd": await _best_slice_price(
            db,
            market_product_code=order.market_product,
            delivery_point_id=order.delivery_point_id,
            availability_window_code=order.availability_window,
            side=order.side,
        ),
    }

# ---------------------------------------------------------------------------
# 1. POST / -- Hit an order to create a trade
# ---------------------------------------------------------------------------

@router.post("/", response_model=TradeResponse)
@retry_market_transaction()
async def create_trade(
    payload: TradeCreate,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization to trade",
        )

    idempotency_key = request.headers.get("Idempotency-Key")
    request_hash = None
    if idempotency_key:
        idempotency_key = idempotency_key.strip()
        if not idempotency_key or len(idempotency_key) > 255:
            raise HTTPException(status_code=400, detail="Idempotency-Key must be 1-255 characters")
        request_hash = idempotency_request_hash(payload.model_dump(mode="json"))
        await acquire_idempotency_lock(db, tenant_id=current_user.organization_id, operation=TRADE_CREATE_OPERATION, key=idempotency_key)
        replay = (await db.execute(
            select(Trade).options(selectinload(Trade.buyer), selectinload(Trade.seller)).where(
                Trade.initiator_org_id == current_user.organization_id,
                Trade.idempotency_operation == TRADE_CREATE_OPERATION,
                Trade.idempotency_key == idempotency_key,
            )
        )).scalar_one_or_none()
        if replay is not None:
            if replay.idempotency_request_hash != request_hash:
                raise HTTPException(status_code=409, detail="Idempotency-Key was reused with a different request")
            return build_trade_response(replay)

    # The canonical market lock is always acquired before the target row lock.
    target_identity = await db.execute(
        select(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.availability_window,
            OrderBookOrder.organization_id,
        ).where(OrderBookOrder.id == payload.order_id)
    )
    target_slice = target_identity.one_or_none()
    if target_slice is not None:
        await acquire_market_slice_lock(db, side=OrderSide.BID, product_id=target_slice.product_id, delivery_point_id=target_slice.delivery_point_id, availability_window=target_slice.availability_window)
    stmt = (
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(OrderBookOrder.id == payload.order_id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    result = await db.execute(stmt)
    order = result.unique().scalar_one_or_none()

    if order is None:
        raise HTTPException(status_code=404, detail="Order not found")
    if target_slice is not None and (
        order.product_id != target_slice.product_id
        or order.delivery_point_id != target_slice.delivery_point_id
        or order.availability_window != target_slice.availability_window
    ):
        await db.rollback()
        raise HTTPException(status_code=409, detail="Order slice changed; retry the trade")
    organizations = await lock_and_load_market_organizations(
        db,
        [current_user.organization_id, order.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(status_code=400, detail="Order is not available for trading")

    if order.expires_at and order.expires_at <= datetime.now(UTC):
        raise HTTPException(status_code=400, detail="Order has expired")

    if not order_is_execution_qualified(order):
        raise HTTPException(status_code=400, detail="Order is not execution-qualified")

    if is_demo_market_organization(order.organization_id):
        raise HTTPException(
            status_code=400,
            detail="Demo listings are preview liquidity and cannot be traded.",
        )

    if not order.owner_user_id:
        raise HTTPException(status_code=409, detail="Order owner requires explicit provenance review")

    target_provenance = coerce_provenance(getattr(order, "provenance", None))
    if target_provenance in {
        OrganizationProvenance.DEMO,
        OrganizationProvenance.TEST,
        OrganizationProvenance.CANARY,
    }:
        raise HTTPException(status_code=400, detail="Synthetic listings cannot be traded")

    # Determine sides
    if order.side == OrderSide.ASK:
        # User is the BUYER hitting a seller's ask
        buyer_org_id = current_user.organization_id
        seller_org_id = order.organization_id
        initiated_by = Initiator.BUYER
        ask_order_id = order.id
        bid_order_id = None
        counterparty_org_id = seller_org_id

        if current_user.role != UserRole.BUYER:
            raise HTTPException(
                status_code=403,
                detail="Only buyers can hit an ASK order",
            )
    else:
        # order.side == BID -- User is the SELLER hitting a buyer's bid
        buyer_org_id = order.organization_id
        seller_org_id = current_user.organization_id
        initiated_by = Initiator.SELLER
        bid_order_id = order.id
        ask_order_id = None
        counterparty_org_id = buyer_org_id

        if current_user.role != UserRole.SUPPLIER:
            raise HTTPException(
                status_code=403,
                detail="Only suppliers can hit a BID order",
            )

    # Prevent self-trade
    if order.organization_id == current_user.organization_id:
        raise HTTPException(status_code=400, detail="Cannot trade with your own order")

    initiator_org = organizations.get(current_user.organization_id)
    initiator_provenance = snapshot_organization_provenance(initiator_org)
    if not execution_provenance_compatible(
        initiator_provenance,
        target_provenance,
        left_org_id=current_user.organization_id,
        right_org_id=order.organization_id,
    ):
        raise HTTPException(status_code=400, detail="Synthetic and live trades cannot be mixed")

    # Re-read and lock both concrete parties and exact organizations in the
    # order transaction. The dependency's earlier admission decision is not
    # trusted at execution time.
    party_result = await db.execute(
        select(User)
        .where(User.id.in_([current_user.id, order.owner_user_id]))
        .order_by(User.id)
        .with_for_update()
        # current_user is already in the session identity map; refresh it
        # under the lock so a mid-request rejection is observed.
        .execution_options(populate_existing=True)
    )
    parties = {party.id: party for party in party_result.scalars().all()}
    org_result = await db.execute(
        select(Organization)
        .where(Organization.id.in_([current_user.organization_id, order.organization_id]))
        .order_by(Organization.id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    locked_organizations = {organization.id: organization for organization in org_result.scalars().all()}
    if not await execution_party_is_eligible(
        db,
        user=parties.get(current_user.id),
        organization=locked_organizations.get(current_user.organization_id),
    ) or not await execution_party_is_eligible(
        db,
        user=parties.get(order.owner_user_id),
        organization=locked_organizations.get(order.organization_id),
    ):
        # Deliberately identical to the generic unavailable-order response:
        # a distinct message here would let any eligible caller probe a
        # counterparty's rejection status by attempting a trade.
        raise HTTPException(status_code=400, detail="Order is not available for trading")

    # Quantity check
    if payload.quantity_mt > order.remaining_quantity_mt:
        raise HTTPException(
            status_code=400,
            detail=f"Requested quantity ({payload.quantity_mt}) exceeds remaining ({order.remaining_quantity_mt})",
        )

    before_state = await _watchlist_before_state(db, order)

    # Create the trade
    trade = Trade(
        bid_order_id=bid_order_id,
        ask_order_id=ask_order_id,
        buyer_id=buyer_org_id,
        seller_id=seller_org_id,
        buyer_user_id=current_user.id if initiated_by == Initiator.BUYER else order.owner_user_id,
        seller_user_id=current_user.id if initiated_by == Initiator.SELLER else order.owner_user_id,
        initiator_org_id=current_user.organization_id,
        buyer_provenance=(target_provenance if order.side == OrderSide.BID else initiator_provenance),
        seller_provenance=(initiator_provenance if order.side == OrderSide.BID else target_provenance),
        initiated_by=initiated_by,
        quantity_mt=payload.quantity_mt,
        price_per_mt_usd=order.price_per_mt_usd,
        status=TradeStatus.PENDING_CONFIRMATION,
        idempotency_key=idempotency_key,
        idempotency_operation=(TRADE_CREATE_OPERATION if idempotency_key else None),
        idempotency_request_hash=request_hash,
        product_id=order.product_id,
        product_name=order.product_name,
        fuel_type=order.fuel_type,
        fuel_grade=order.fuel_grade,
        market_product=order.market_product,
        delivery_point_id=order.delivery_point_id,
        delivery_point_name=order.delivery_point_name,
        delivery_point_region=order.region,
        availability_window=order.availability_window,
    )
    db.add(trade)

    # Update order remaining quantity and status
    order.remaining_quantity_mt -= payload.quantity_mt

    if order.remaining_quantity_mt == Decimal("0"):
        order.status = OrderBookStatus.FILLED
    elif order.status == OrderBookStatus.OPEN:
        order.status = OrderBookStatus.PARTIALLY_FILLED

    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [(order.side, order.market_product, order.delivery_point_id, order.availability_window)],
    )

    # Flush is the actual INSERT point. Resolve a concurrent unique-key
    # winner here, while the transaction-scoped idempotency lock is still the
    # source of serialization.
    try:
        await db.flush()
    except IntegrityError:
        await db.rollback()
        if not idempotency_key:
            raise
        existing_result = await db.execute(
            select(Trade)
            .options(
                selectinload(Trade.buyer), selectinload(Trade.seller),
                selectinload(Trade.bid_order).selectinload(OrderBookOrder.product),
                selectinload(Trade.ask_order).selectinload(OrderBookOrder.product),
            )
            .where(
                Trade.initiator_org_id == current_user.organization_id,
                Trade.idempotency_operation == TRADE_CREATE_OPERATION,
                Trade.idempotency_key == idempotency_key,
            )
        )
        existing_trade = existing_result.unique().scalar_one_or_none()
        if existing_trade is None:
            raise
        if existing_trade.idempotency_request_hash != request_hash:
            raise HTTPException(status_code=409, detail="Idempotency-Key was reused with a different request")
        return build_trade_response(existing_trade)
    await emit_order_updated(db, before=before_state, order=order)

    # Notify counterparty organization
    await notify_org_users(
        db,
        counterparty_org_id,
        NotificationType.ORDER_UPDATE,
        "New Trade Request",
        f"A new trade request for {payload.quantity_mt} MT at ${order.price_per_mt_usd}/MT has been placed.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_CREATED,
        resource_type="trade",
        resource_id=trade.id,
        changes={
            "order_id": str(order.id),
            "quantity_mt": str(payload.quantity_mt),
            "price_per_mt_usd": str(order.price_per_mt_usd),
        },
        **request_audit_context(request),
    )
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="trade_created",
                aggregate_type="trade",
                aggregate_id=trade.id,
                participant_org_ids=(trade.buyer_id, trade.seller_id),
                payload={
                    **trade_activity_provenance(trade),
                    "id": str(trade.id),
                    "status": trade.status.value,
                    "quantity": str(trade.quantity_mt),
                    "price": str(trade.price_per_mt_usd),
                    "product_name": trade.product_name or "",
                    "fuel_type": trade.fuel_type or "",
                    "region": trade.delivery_point_region or "",
                },
            )
        ],
    )
    await db.commit()
    track_analytics_event(
        trade_created_event(current_user, order=order, request=request), request=request
    )

    # Reload with relationships for response
    loaded_trade = await _load_trade(db, trade.id)

    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 2. GET /my -- List my trades (as buyer or seller)
# ---------------------------------------------------------------------------

@router.get("/my", response_model=PaginatedResponse[TradeResponse])
async def list_my_trades(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
):
    org_id = current_user.organization_id

    org_filter = or_(Trade.buyer_id == org_id, Trade.seller_id == org_id)

    # Count query (same filter, no pagination)
    count_query = select(func.count(Trade.id)).where(org_filter)
    total = (await db.execute(count_query)).scalar()

    # Data query with pagination
    stmt = (
        select(Trade)
        .where(org_filter)
        .options(
            joinedload(Trade.buyer),
            joinedload(Trade.seller),
            joinedload(Trade.bid_order),
            joinedload(Trade.ask_order),
        )
        .order_by(Trade.created_at.desc())
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    trades = result.unique().scalars().all()

    items = [build_trade_response(t) for t in trades]
    return PaginatedResponse(items=items, total=total, skip=skip, limit=limit)


# ---------------------------------------------------------------------------
# 3. PUT /{trade_id}/confirm -- Counterparty confirms the trade
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/confirm", response_model=TradeResponse)
@retry_market_transaction()
async def confirm_trade(
    trade_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    trade, confirmed_order = await _lock_trade_market_rows(
        db, trade_id, operation="trade_confirm"
    )
    await lock_and_load_market_organizations(
        db,
        [trade.buyer_id, trade.seller_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )
    org_id = current_user.organization_id

    if trade.status != TradeStatus.PENDING_CONFIRMATION:
        raise HTTPException(status_code=400, detail="Trade is not pending confirmation")
    if not execution_provenance_compatible(
        trade.buyer_provenance,
        trade.seller_provenance,
        left_org_id=trade.buyer_id,
        right_org_id=trade.seller_id,
    ):
        raise HTTPException(
            status_code=409,
            detail="Trade provenance is not eligible for execution",
        )

    # Only the counterparty (non-initiator) can confirm
    if trade.initiated_by == Initiator.BUYER:
        # Buyer initiated, so seller confirms
        if trade.seller_id != org_id or trade.seller_user_id != current_user.id:
            raise HTTPException(status_code=403, detail="Only the seller can confirm this trade")
        initiator_org_id = trade.buyer_id
    else:
        # Seller initiated, so buyer confirms
        if trade.buyer_id != org_id or trade.buyer_user_id != current_user.id:
            raise HTTPException(status_code=403, detail="Only the buyer can confirm this trade")
        initiator_org_id = trade.seller_id

    await _revalidate_trade_parties(db, trade)

    trade.status = TradeStatus.CONFIRMED
    trade.confirmed_at = datetime.now(UTC)
    if confirmed_order is not None and confirmed_order.inventory_item_id is not None:
        await consume_inventory(db, confirmed_order.inventory_item_id, trade.quantity_mt)

    # Progress referrals to ACTIVE for users in buyer/seller orgs (single query)
    from app.models.referral import Referral, ReferralStatus
    ref_stmt = (
        select(Referral)
        .join(User, Referral.referred_user_id == User.id)
        .where(
            User.organization_id.in_([trade.buyer_id, trade.seller_id]),
            Referral.status == ReferralStatus.VERIFIED,
        )
    )
    now = datetime.now(UTC)
    for referral in (await db.execute(ref_stmt)).scalars():
        referral.status = ReferralStatus.ACTIVE
        referral.activated_at = now

    # Notify the initiator
    await notify_org_users(
        db,
        initiator_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Confirmed",
        f"Your trade for {trade.quantity_mt} MT has been confirmed by the counterparty.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_CONFIRMED,
        resource_type="trade",
        resource_id=trade.id,
        changes={"status": TradeStatus.CONFIRMED.value},
        **request_audit_context(request),
    )
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="trade_confirmed",
                aggregate_type="trade",
                aggregate_id=trade.id,
                participant_org_ids=(trade.buyer_id, trade.seller_id),
                payload={
                    **trade_activity_provenance(trade),
                    "id": str(trade.id),
                    "status": trade.status.value,
                    "quantity": str(trade.quantity_mt),
                    "price": str(trade.price_per_mt_usd),
                },
            )
        ],
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)

    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 4. PUT /{trade_id}/decline -- Counterparty declines the trade
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/decline", response_model=TradeResponse)
@retry_market_transaction()
async def decline_trade(
    trade_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_authenticated_user)],
):
    # Load without a row lock; decline takes the market-slice lock before
    # locking either the trade or its order.
    trade = await _load_trade(db, trade_id, for_update=False)
    org_id = current_user.organization_id

    if trade.status != TradeStatus.PENDING_CONFIRMATION:
        raise HTTPException(status_code=400, detail="Trade is not pending confirmation")

    # Only the counterparty (non-initiator) can decline
    if trade.initiated_by == Initiator.BUYER:
        is_counterparty = (
            trade.seller_user_id == current_user.id
            if trade.seller_user_id is not None
            else trade.seller_id == org_id
        )
        if not is_counterparty:
            raise HTTPException(status_code=403, detail="Only the seller can decline this trade")
        initiator_org_id = trade.buyer_id
    else:
        is_counterparty = (
            trade.buyer_user_id == current_user.id
            if trade.buyer_user_id is not None
            else trade.buyer_id == org_id
        )
        if not is_counterparty:
            raise HTTPException(status_code=403, detail="Only the buyer can decline this trade")
        initiator_org_id = trade.seller_id

    trade, order = await _lock_trade_market_rows(
        db, trade_id, operation="trade_decline"
    )
    if trade.status != TradeStatus.PENDING_CONFIRMATION:
        await db.rollback()
        raise HTTPException(status_code=400, detail="Trade is not pending confirmation")
    locked_slice = _trade_market_slice(trade)
    locked_identity = (
        (locked_slice[1], locked_slice[2], locked_slice[3])
        if locked_slice is not None
        else None
    )
    await lock_and_load_market_organizations(
        db,
        [trade.buyer_id, trade.seller_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )
    trade.status = TradeStatus.DECLINED

    # Restore the order's remaining quantity. The eager-loaded relationship
    # row is not covered by the trade's FOR UPDATE lock, so re-select it
    # locked before mutating remaining_quantity_mt.
    before_state = None
    if order is not None:
        if locked_identity != (order.product_id, order.delivery_point_id, order.availability_window):
            await db.rollback()
            raise HTTPException(status_code=409, detail="Order slice changed; retry the decline")
        if order.status not in (OrderBookStatus.CANCELLED, OrderBookStatus.EXPIRED):
            before_state = await _watchlist_before_state(db, order)
        if order.status in (OrderBookStatus.CANCELLED, OrderBookStatus.EXPIRED):
            # The resting order was withdrawn while the trade was pending;
            # declining must not revive it as live liquidity.
            if order.inventory_item_id is not None:
                await release_inventory(db, order.inventory_item_id, trade.quantity_mt)
            order = None
        elif order.expires_at is not None and order.expires_at <= datetime.now(UTC):
            # Restore the declined quantity for history, but release the full
            # now-unexecutable balance instead of reviving stale liquidity.
            order.remaining_quantity_mt += trade.quantity_mt
            if order.inventory_item_id is not None and order.remaining_quantity_mt > 0:
                await release_inventory(
                    db,
                    order.inventory_item_id,
                    order.remaining_quantity_mt,
                )
            order.status = OrderBookStatus.EXPIRED
            await rebuild_live_slice_benchmarks_for_keys(
                db,
                [(order.side, order.market_product, order.delivery_point_id, order.availability_window)],
            )
            if before_state is not None:
                await emit_order_updated(db, before=before_state, order=order)
            await record_audit(
                db,
                user_id=current_user.id,
                action=ORDER_EXPIRED,
                resource_type="order",
                resource_id=order.id,
                changes={
                    "status": OrderBookStatus.EXPIRED.value,
                    "released_quantity_mt": str(order.remaining_quantity_mt),
                },
                **request_audit_context(request),
            )
            order = None
    if order is not None:
        order.remaining_quantity_mt += trade.quantity_mt

        if order.remaining_quantity_mt == order.quantity_mt:
            order.status = OrderBookStatus.OPEN
        else:
            order.status = OrderBookStatus.PARTIALLY_FILLED
        await rebuild_live_slice_benchmarks_for_keys(
            db,
            [(order.side, order.market_product, order.delivery_point_id, order.availability_window)],
        )
        if before_state is not None:
            await emit_order_updated(db, before=before_state, order=order)

    # Notify the initiator
    await notify_org_users(
        db,
        initiator_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Declined",
        f"Your trade request for {trade.quantity_mt} MT has been declined.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_DECLINED,
        resource_type="trade",
        resource_id=trade.id,
        changes={"status": TradeStatus.DECLINED.value},
        **request_audit_context(request),
    )
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="trade_declined",
                aggregate_type="trade",
                aggregate_id=trade.id,
                participant_org_ids=(trade.buyer_id, trade.seller_id),
                payload={
                    **trade_activity_provenance(trade),
                    "id": str(trade.id),
                    "status": trade.status.value,
                },
            )
        ],
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)
    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 5. PUT /{trade_id}/deliver -- Mark trade as delivered with final qty/price
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/deliver", response_model=TradeResponse)
@retry_market_transaction()
async def deliver_trade(
    trade_id: UUID,
    payload: TradeDeliverPayload,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    trade, _linked_order = await _lock_trade_market_rows(
        db, trade_id, operation="trade_deliver"
    )
    org_id = current_user.organization_id
    await lock_and_load_market_organizations(
        db,
        [trade.buyer_id, trade.seller_id],
        actor_ownerships=(MarketActorOwnership(current_user.id, org_id),),
    )

    if trade.status != TradeStatus.CONFIRMED:
        raise HTTPException(status_code=400, detail="Trade must be confirmed before delivery")

    # Either party can mark as delivered
    if (
        (org_id, current_user.id)
        not in (
            (trade.buyer_id, trade.buyer_user_id),
            (trade.seller_id, trade.seller_user_id),
        )
    ):
        raise HTTPException(status_code=403, detail="Not authorized for this trade")
    await _revalidate_trade_parties(db, trade)
    if payload.final_quantity_mt > trade.quantity_mt:
        raise HTTPException(
            status_code=400,
            detail="final_quantity_mt cannot exceed originally traded quantity",
        )
    price_deviation_pct = (
        abs(payload.final_price_per_mt - trade.price_per_mt_usd)
        / trade.price_per_mt_usd
        * Decimal("100")
    )
    if price_deviation_pct > MAX_FINAL_PRICE_DEVIATION_PCT:
        raise HTTPException(
            status_code=400,
            detail=(
                f"final_price_per_mt deviates more than {MAX_FINAL_PRICE_DEVIATION_PCT}% "
                "from the confirmed trade price"
            ),
        )

    trade.final_quantity_mt = payload.final_quantity_mt
    trade.final_price_per_mt = payload.final_price_per_mt
    trade.final_total_usd = payload.final_quantity_mt * payload.final_price_per_mt
    trade.delivered_at = datetime.now(UTC)
    trade.status = TradeStatus.DELIVERED

    # Calculate commission on the trade (stored on the Trade record itself for Phase 1)
    trade.commission_amount_usd = trade.final_total_usd * (trade.commission_rate_pct / Decimal("100"))

    # Notify the other party
    counterparty_org_id = trade.seller_id if org_id == trade.buyer_id else trade.buyer_id

    await notify_org_users(
        db,
        counterparty_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Delivered",
        f"Trade for {trade.final_quantity_mt} MT has been marked as delivered.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_DELIVERED,
        resource_type="trade",
        resource_id=trade.id,
        changes={
            "status": TradeStatus.DELIVERED.value,
            "final_quantity_mt": str(trade.final_quantity_mt),
            "final_price_per_mt": str(trade.final_price_per_mt),
            "final_total_usd": str(trade.final_total_usd),
        },
        **request_audit_context(request),
    )
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="trade_delivered",
                aggregate_type="trade",
                aggregate_id=trade.id,
                participant_org_ids=(trade.buyer_id, trade.seller_id),
                payload={
                    **trade_activity_provenance(trade),
                    "id": str(trade.id),
                    "status": trade.status.value,
                    "final_quantity": str(trade.final_quantity_mt),
                    "final_price": str(trade.final_price_per_mt),
                    "final_total": str(trade.final_total_usd),
                },
            )
        ],
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)

    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 6. POST /{trade_id}/pay -- Mark trade as paid
# ---------------------------------------------------------------------------

@router.post("/{trade_id}/pay", response_model=TradeResponse)
@retry_market_transaction()
async def pay_trade(
    trade_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    trade, _linked_order = await _lock_trade_market_rows(
        db, trade_id, operation="trade_pay"
    )
    org_id = current_user.organization_id
    await lock_and_load_market_organizations(
        db,
        [trade.buyer_id, trade.seller_id],
        actor_ownerships=(MarketActorOwnership(current_user.id, org_id),),
    )

    if trade.status != TradeStatus.DELIVERED:
        raise HTTPException(status_code=400, detail="Trade must be delivered before payment")

    # Only the seller can mark as paid
    if trade.seller_id != org_id or trade.seller_user_id != current_user.id:
        raise HTTPException(status_code=403, detail="Only the seller can mark a trade as paid")

    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can mark trades as paid")
    await _revalidate_trade_parties(db, trade)

    trade.status = TradeStatus.PAID
    trade.paid_at = datetime.now(UTC)

    # Notify the buyer
    await notify_org_users(
        db,
        trade.buyer_id,
        NotificationType.ORDER_UPDATE,
        "Trade Paid",
        f"Payment for trade of {trade.quantity_mt} MT has been confirmed.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_PAID,
        resource_type="trade",
        resource_id=trade.id,
        changes={"status": TradeStatus.PAID.value},
        **request_audit_context(request),
    )
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="trade_paid",
                aggregate_type="trade",
                aggregate_id=trade.id,
                participant_org_ids=(trade.buyer_id, trade.seller_id),
                payload={
                    **trade_activity_provenance(trade),
                    "id": str(trade.id),
                    "status": trade.status.value,
                    "quantity": str(trade.quantity_mt),
                    "price": str(trade.price_per_mt_usd),
                },
            )
        ],
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)

    return build_trade_response(loaded_trade)

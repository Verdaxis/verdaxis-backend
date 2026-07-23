"""
Match-on-insert engine. When a new order is placed, scan for crossing orders
and automatically create trades. Uses price-time priority (FIFO at each price level).
"""
from datetime import datetime, UTC
import uuid

from fastapi import HTTPException
from sqlalchemy import select, or_
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import (
    OrderBookOrder, Trade, OrderSide, OrderBookStatus, TradeStatus, Initiator
)
from app.models.notification import NotificationType
from app.models.user import Organization, User
from app.services.demo_market import (
    DEMO_ACTIVITY_BUYER_ORG_ID,
    DEMO_ACTIVITY_ORG_IDS,
    DEMO_ACTIVITY_SELLER_ORG_ID,
)
from app.services.execution_policy import (
    execution_party_is_eligible,
    order_owner_is_execution_eligible,
    order_is_execution_qualified,
    orders_execution_compatible,
)
from app.services.market_locks import acquire_market_slice_lock
from app.services.provenance import coerce_provenance, execution_provenance_compatible
from app.models.user import OrganizationProvenance
from app.services.inventory_reservations import lock_inventory_items
from app.services.market_admission import lock_and_load_market_organizations
from app.services.org_notifications import OrgNotification, notify_org_users_batched

# A single transaction must not hold an unbounded number of market rows. This
# is a conservative operational cap: callers can retry the remainder in a
# later transaction, while the first 100 price-time candidates retain the
# existing matching semantics.
MAX_CROSSING_ORDERS_PER_MATCH = 100


async def match_order(
    db: AsyncSession,
    new_order: OrderBookOrder,
    is_anonymous: bool = False,
    allowed_demo_order_pair: frozenset[uuid.UUID] | None = None,
) -> list[Trade]:
    """
    Attempt to match a newly created order against the opposite side of the book.

    Rules:
    - BID matches against ASKs where ask_price <= bid_price
    - ASK matches against BIDs where bid_price >= ask_price
    - Product, delivery point, availability window, and certification constraints must be compatible
    - Price-time priority: best price first, then oldest order first
    - Partial fills allowed: match as much as possible
    - Self-trade prevention: skip orders from same organization
    - Demo matching requires an exact two-order allowlist and never crosses provenance boundaries
    - All operations within caller's transaction (no separate commit)

    Returns list of Trade objects created (may be empty if no matches).
    """
    trades_created: list[Trade] = []
    pending_notifications: list[OrgNotification] = []

    if new_order.remaining_quantity_mt <= 0:
        return trades_created

    await acquire_market_slice_lock(
        db,
        side=new_order.side,
        product_id=new_order.product_id,
        delivery_point_id=new_order.delivery_point_id,
        availability_window=new_order.availability_window,
    )

    now = datetime.now(UTC)
    if new_order.expires_at is not None and new_order.expires_at <= now:
        new_order.status = OrderBookStatus.EXPIRED
        new_order.bump_version()
        return trades_created

    new_order_provenance = getattr(new_order, "provenance", None) or OrganizationProvenance.UNKNOWN
    new_order_provenance = coerce_provenance(new_order_provenance)
    if new_order_provenance not in {
        OrganizationProvenance.REAL,
        OrganizationProvenance.DEMO,
    }:
        return trades_created
    new_order_is_demo = new_order_provenance == OrganizationProvenance.DEMO
    allowed_demo_counterparty_id: uuid.UUID | None = None
    allowed_demo_counterparty_org_id: uuid.UUID | None = None
    if new_order_is_demo:
        expected_new_org_id = (
            DEMO_ACTIVITY_BUYER_ORG_ID
            if new_order.side == OrderSide.BID
            else DEMO_ACTIVITY_SELLER_ORG_ID
        )
        if new_order.organization_id != expected_new_org_id:
            return trades_created
        if (
            allowed_demo_order_pair is None
            or new_order.id is None
            or len(allowed_demo_order_pair) != 2
            or new_order.id not in allowed_demo_order_pair
        ):
            return trades_created
        counterparty_ids = allowed_demo_order_pair - {new_order.id}
        if len(counterparty_ids) != 1:
            return trades_created
        allowed_demo_counterparty_id = next(iter(counterparty_ids))
        allowed_demo_counterparty_org_id = (
            DEMO_ACTIVITY_SELLER_ORG_ID
            if new_order.side == OrderSide.BID
            else DEMO_ACTIVITY_BUYER_ORG_ID
        )

    if not order_is_execution_qualified(new_order):
        return trades_created

    # Determine which side to match against
    if new_order.side == OrderSide.BID:
        # BID: match against ASKs where ask_price <= bid_price
        opposite_side = OrderSide.ASK
        # Best ask = lowest price first (ascending), then oldest first
        price_order = OrderBookOrder.price_per_mt_usd.asc()
        price_filter = OrderBookOrder.price_per_mt_usd <= new_order.price_per_mt_usd
    else:
        # ASK: match against BIDs where bid_price >= ask_price
        opposite_side = OrderSide.BID
        # Best bid = highest price first (descending), then oldest first
        price_order = OrderBookOrder.price_per_mt_usd.desc()
        price_filter = OrderBookOrder.price_per_mt_usd >= new_order.price_per_mt_usd

    # Build matching filters: same product_id, same delivery_point_id
    match_filters = [
        OrderBookOrder.side == opposite_side,
        OrderBookOrder.product_id == new_order.product_id,
        OrderBookOrder.availability_window == new_order.availability_window,
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        OrderBookOrder.organization_id != new_order.organization_id,  # No self-trade
        price_filter,
        or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > now),
        OrderBookOrder.provenance == new_order_provenance,
        OrderBookOrder.organization.has(
            Organization.verification_status == "APPROVED"
        ),
    ]

    # delivery_point_id match: both NULL or both equal
    if new_order.delivery_point_id is None:
        match_filters.append(OrderBookOrder.delivery_point_id.is_(None))
    else:
        match_filters.append(OrderBookOrder.delivery_point_id == new_order.delivery_point_id)

    # Preview a strictly bounded, already provenance/admission-filtered set.
    candidate_stmt = (
        select(OrderBookOrder.id, OrderBookOrder.organization_id)
        .where(*match_filters)
        .order_by(price_order, OrderBookOrder.created_at.asc())  # Price-time priority
        .limit(MAX_CROSSING_ORDERS_PER_MATCH + 1)
    )
    candidate_rows = (await db.execute(candidate_stmt)).all()
    fanout_exceeded = len(candidate_rows) > MAX_CROSSING_ORDERS_PER_MATCH
    candidate_rows = candidate_rows[:MAX_CROSSING_ORDERS_PER_MATCH]
    if not candidate_rows:
        return trades_created

    # Reapply every predicate while locking the exact previewed rows. The
    # slice advisory lock makes the price-time set stable between both reads.
    candidate_ids = [row.id for row in candidate_rows]
    stmt = (
        select(OrderBookOrder)
        .where(OrderBookOrder.id.in_(candidate_ids), *match_filters)
        .order_by(price_order, OrderBookOrder.created_at.asc())
        .with_for_update()
        .execution_options(populate_existing=True)
    )

    result = await db.execute(stmt)
    crossing_orders = result.scalars().all()
    await lock_and_load_market_organizations(
        db,
        [new_order.organization_id, *(row.organization_id for row in crossing_orders)],
    )

    for crossing in crossing_orders:
        if new_order.remaining_quantity_mt <= 0:
            break
        crossing_provenance = getattr(crossing, "provenance", None) or OrganizationProvenance.UNKNOWN
        crossing_provenance = coerce_provenance(crossing_provenance)
        if crossing.expires_at is not None and crossing.expires_at <= now:
            crossing.status = OrderBookStatus.EXPIRED
            crossing.bump_version()
            continue
        if not execution_provenance_compatible(
            new_order_provenance,
            crossing_provenance,
            left_org_id=new_order.organization_id,
            right_org_id=crossing.organization_id,
            allowed_demo_org_pair=DEMO_ACTIVITY_ORG_IDS,
        ):
            continue
        if allowed_demo_counterparty_id is not None:
            if crossing.id != allowed_demo_counterparty_id or crossing.organization_id != allowed_demo_counterparty_org_id:
                continue
        if not orders_execution_compatible(new_order, crossing):
            continue

        # Both concrete order creators must still be admitted at the moment
        # the match is committed. Legacy rows without an owner fail closed.
        # The sanctioned demo-activity pair is synthetic DEMO-provenance
        # liquidity with no concrete owner; provenance isolation above already
        # prevents it from ever crossing a live order.
        if allowed_demo_counterparty_id is None:
            if not new_order.owner_user_id or not crossing.owner_user_id:
                continue
            # populate_existing: these party rows may already sit in the
            # session identity map from the request preview; the locked
            # SELECT must observe a concurrent rejection, not stale state.
            owners_result = await db.execute(
                select(User)
                .where(User.id.in_([new_order.owner_user_id, crossing.owner_user_id]))
                .order_by(User.id)
                .with_for_update()
                .execution_options(populate_existing=True)
            )
            owners = {user.id: user for user in owners_result.scalars().all()}
            orgs_result = await db.execute(
                select(Organization)
                .where(
                    Organization.id.in_([new_order.organization_id, crossing.organization_id])
                )
                .order_by(Organization.id)
                .with_for_update()
                .execution_options(populate_existing=True)
            )
            orgs = {org.id: org for org in orgs_result.scalars().all()}
            if not await order_owner_is_execution_eligible(
                db,
                order=new_order,
                user=owners.get(new_order.owner_user_id),
                organization=orgs.get(new_order.organization_id),
            ) or not await order_owner_is_execution_eligible(
                db,
                order=crossing,
                user=owners.get(crossing.owner_user_id),
                organization=orgs.get(crossing.organization_id),
            ):
                continue

        # Determine trade quantity (minimum of both remaining quantities)
        trade_qty = min(new_order.remaining_quantity_mt, crossing.remaining_quantity_mt)

        # Inventory rows participate in the same deterministic lifecycle
        # ledger as order quantities. Lock all affected rows by UUID before
        # consuming any reserved quantity.
        consumption_by_item = {}
        for source_order in (new_order, crossing):
            item_id = getattr(source_order, "inventory_item_id", None)
            if item_id is not None:
                consumption_by_item[item_id] = consumption_by_item.get(item_id, 0) + trade_qty
        inventory_ids = set(consumption_by_item)
        inventory_rows = await lock_inventory_items(db, inventory_ids)
        for item_id, consumed in consumption_by_item.items():
            item = inventory_rows[item_id]
            reserved = item.reserved_stock_mt or 0
            if reserved < consumed:
                raise RuntimeError("inventory reservation is inconsistent; refusing the match")
            item.reserved_stock_mt = reserved - consumed

        # Trade price = the resting order's price (price improvement for aggressor)
        trade_price = crossing.price_per_mt_usd

        # Determine buyer/seller
        if new_order.side == OrderSide.BID:
            buyer_org = new_order.organization_id
            seller_org = crossing.organization_id
            bid_order_id = new_order.id
            ask_order_id = crossing.id
            initiated_by = Initiator.BUYER
        else:
            buyer_org = crossing.organization_id
            seller_org = new_order.organization_id
            bid_order_id = crossing.id
            ask_order_id = new_order.id
            initiated_by = Initiator.SELLER

        # Create trade (auto-confirmed since both sides agreed via price)
        trade = Trade(
            bid_order_id=bid_order_id,
            ask_order_id=ask_order_id,
            buyer_id=buyer_org,
            seller_id=seller_org,
            buyer_user_id=new_order.owner_user_id if new_order.side == OrderSide.BID else crossing.owner_user_id,
            seller_user_id=crossing.owner_user_id if new_order.side == OrderSide.BID else new_order.owner_user_id,
            initiator_org_id=(new_order.organization_id),
            buyer_provenance=(new_order_provenance if new_order.side == OrderSide.BID else crossing_provenance),
            seller_provenance=(crossing_provenance if new_order.side == OrderSide.BID else new_order_provenance),
            initiated_by=initiated_by,
            quantity_mt=trade_qty,
            price_per_mt_usd=trade_price,
            status=TradeStatus.CONFIRMED,  # Auto-matched = auto-confirmed
            confirmed_at=datetime.now(UTC),
            is_anonymous=is_anonymous,
            product_id=new_order.product_id,
            product_name=new_order.product_name,
            fuel_type=new_order.fuel_type,
            fuel_grade=new_order.fuel_grade,
            market_product=new_order.market_product,
            delivery_point_id=new_order.delivery_point_id,
            delivery_point_name=new_order.delivery_point_name,
            delivery_point_region=new_order.region,
            availability_window=new_order.availability_window,
        )
        db.add(trade)
        await db.flush()

        # Update quantities
        new_order.remaining_quantity_mt -= trade_qty
        crossing.remaining_quantity_mt -= trade_qty

        # Update order statuses
        if new_order.remaining_quantity_mt == 0:
            new_order.status = OrderBookStatus.FILLED
        else:
            new_order.status = OrderBookStatus.PARTIALLY_FILLED

        if crossing.remaining_quantity_mt == 0:
            crossing.status = OrderBookStatus.FILLED
        else:
            crossing.status = OrderBookStatus.PARTIALLY_FILLED
        new_order.bump_version()
        crossing.bump_version()

        trades_created.append(trade)

        # Derive product name for notification messages
        product_name = new_order.product_name or "fuel"

        # Queue notifications for both parties; delivered in one batch after
        # the loop so we run a single user query instead of two per trade.
        message = (
            f"Your order was automatically matched: "
            f"{trade_qty} MT of {product_name} at ${trade_price}/MT"
        )
        data = {"trade_id": str(trade.id), "auto_matched": True}
        pending_notifications.append(
            (buyer_org, NotificationType.TRADE_CONFIRMED, "Auto-Matched Trade", message, data)
        )
        pending_notifications.append(
            (seller_org, NotificationType.TRADE_CONFIRMED, "Auto-Matched Trade", message, data)
        )

    await notify_org_users_batched(db, pending_notifications)

    # Session autoflush is disabled. Persist final order quantities/statuses
    # before callers rebuild query-derived benchmarks and event projections.
    await db.flush()

    if fanout_exceeded and new_order.remaining_quantity_mt > 0:
        raise HTTPException(
            status_code=409,
            detail=(
                "Market slice match fan-out exceeds the per-transaction bound; "
                "no partial matching result was committed"
            ),
        )

    return trades_created

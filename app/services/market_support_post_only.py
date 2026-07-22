"""Post-only assessment using the matcher's executable-candidate semantics."""
from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from decimal import Decimal

from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrganizationProvenance, User
from app.services.execution_policy import (
    execution_party_is_eligible,
    order_is_execution_qualified,
    orders_execution_compatible,
)
from app.services.market_admission import MarketActorOwnership, lock_and_load_market_organizations
from app.services.market_locks import acquire_market_slice_lock
from app.services.matching_engine import MAX_CROSSING_ORDERS_PER_MATCH
from app.services.provenance import coerce_provenance, execution_provenance_compatible


@dataclass(frozen=True)
class PostOnlyAssessment:
    would_cross: bool
    indeterminate: bool = False
    best_executable_opposing_price_per_mt_usd: Decimal | None = None
    raw_candidate_count: int = 0


async def assess_post_only(
    db: AsyncSession,
    candidate: OrderBookOrder,
    *,
    accountable_user: User,
    organization: Organization,
    lock: bool,
) -> PostOnlyAssessment:
    """Return whether ``candidate`` would execute immediately.

    The final publication path calls this with ``lock=True``. It holds the same
    transaction-scoped market-slice advisory lock used by matching, locks the
    exact candidate orders, and revalidates concrete owners. The subsequent
    ordinary order-creation/update pipeline therefore observes a stable book.
    """
    if candidate.remaining_quantity_mt <= 0 or not order_is_execution_qualified(candidate):
        return PostOnlyAssessment(would_cross=False)

    provenance = coerce_provenance(
        getattr(candidate, "provenance", None) or OrganizationProvenance.UNKNOWN
    )
    if provenance not in {OrganizationProvenance.REAL, OrganizationProvenance.DEMO}:
        return PostOnlyAssessment(would_cross=False)

    if lock:
        await acquire_market_slice_lock(
            db,
            side=candidate.side,
            product_id=candidate.product_id,
            delivery_point_id=candidate.delivery_point_id,
            availability_window=candidate.availability_window,
        )

    now = datetime.now(UTC)
    if candidate.side == OrderSide.BID:
        opposite_side = OrderSide.ASK
        price_filter = OrderBookOrder.price_per_mt_usd <= candidate.price_per_mt_usd
        price_order = OrderBookOrder.price_per_mt_usd.asc()
    else:
        opposite_side = OrderSide.BID
        price_filter = OrderBookOrder.price_per_mt_usd >= candidate.price_per_mt_usd
        price_order = OrderBookOrder.price_per_mt_usd.desc()

    filters: list[object] = [
        OrderBookOrder.side == opposite_side,
        OrderBookOrder.product_id == candidate.product_id,
        OrderBookOrder.availability_window == candidate.availability_window,
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        OrderBookOrder.organization_id != candidate.organization_id,
        price_filter,
        or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > now),
        OrderBookOrder.provenance == provenance,
        OrderBookOrder.organization.has(Organization.verification_status == "APPROVED"),
    ]
    if candidate.delivery_point_id is None:
        filters.append(OrderBookOrder.delivery_point_id.is_(None))
    else:
        filters.append(OrderBookOrder.delivery_point_id == candidate.delivery_point_id)

    statement = (
        select(OrderBookOrder)
        .where(*filters)
        .order_by(price_order, OrderBookOrder.created_at.asc())
        .limit(MAX_CROSSING_ORDERS_PER_MATCH + 1)
    )
    if lock:
        statement = statement.with_for_update().execution_options(populate_existing=True)
    crossing_orders = list((await db.execute(statement)).scalars().all())
    raw_candidate_count = len(crossing_orders)
    if raw_candidate_count > MAX_CROSSING_ORDERS_PER_MATCH:
        # Matching itself refuses an unbounded fan-out. A delegated action
        # must fail closed rather than publish into an indeterminate slice.
        return PostOnlyAssessment(
            would_cross=True,
            indeterminate=True,
            raw_candidate_count=raw_candidate_count,
        )
    if not crossing_orders:
        return PostOnlyAssessment(would_cross=False, raw_candidate_count=0)

    if lock:
        await lock_and_load_market_organizations(
            db,
            [organization.id],
            actor_ownerships=(MarketActorOwnership(accountable_user.id, organization.id),),
        )
    elif not await execution_party_is_eligible(
        db, user=accountable_user, organization=organization
    ):
        return PostOnlyAssessment(would_cross=False, raw_candidate_count=raw_candidate_count)

    owner_ids = sorted(
        {order.owner_user_id for order in crossing_orders if order.owner_user_id is not None},
        key=str,
    )
    owner_statement = select(User).where(User.id.in_(owner_ids)).order_by(User.id)
    if lock:
        owner_statement = owner_statement.with_for_update().execution_options(populate_existing=True)
    owners = {
        user.id: user for user in (await db.execute(owner_statement)).scalars().all()
    } if owner_ids else {}

    organization_ids = sorted({order.organization_id for order in crossing_orders}, key=str)
    organization_statement = (
        select(Organization)
        .where(Organization.id.in_(organization_ids))
        .order_by(Organization.id)
    )
    if lock:
        organization_statement = organization_statement.with_for_update().execution_options(
            populate_existing=True
        )
    organizations = {
        row.id: row for row in (await db.execute(organization_statement)).scalars().all()
    }

    for crossing in crossing_orders:
        crossing_provenance = coerce_provenance(
            getattr(crossing, "provenance", None) or OrganizationProvenance.UNKNOWN
        )
        if not execution_provenance_compatible(
            provenance,
            crossing_provenance,
            left_org_id=candidate.organization_id,
            right_org_id=crossing.organization_id,
        ):
            continue
        if not orders_execution_compatible(candidate, crossing):
            continue
        if crossing.owner_user_id is None:
            continue
        owner = owners.get(crossing.owner_user_id)
        counterparty_organization = organizations.get(crossing.organization_id)
        if not await execution_party_is_eligible(
            db,
            user=owner,
            organization=counterparty_organization,
        ):
            continue
        return PostOnlyAssessment(
            would_cross=True,
            best_executable_opposing_price_per_mt_usd=crossing.price_per_mt_usd,
            raw_candidate_count=raw_candidate_count,
        )

    return PostOnlyAssessment(
        would_cross=False,
        raw_candidate_count=raw_candidate_count,
    )

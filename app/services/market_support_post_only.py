"""Fail-closed post-only assessment for assisted listings."""
from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from decimal import Decimal

from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrganizationProvenance, User
from app.services.execution_policy import (
    order_owner_is_execution_eligible,
    order_is_execution_qualified,
    orders_execution_compatible,
)
from app.services.matching_engine import MAX_CROSSING_ORDERS_PER_MATCH


@dataclass(frozen=True)
class PostOnlyAssessment:
    would_cross: bool
    indeterminate: bool = False
    best_executable_price: Decimal | None = None


async def assess_locked_order(
    db: AsyncSession,
    candidate: OrderBookOrder,
    *,
    organization: Organization,
) -> PostOnlyAssessment:
    """Assess a REAL assisted order after the caller acquires its market-slice lock."""
    if (
        candidate.remaining_quantity_mt <= 0
        or not order_is_execution_qualified(candidate)
        or candidate.provenance != OrganizationProvenance.REAL
        or organization.id != candidate.organization_id
        or organization.verification_status != "APPROVED"
    ):
        return PostOnlyAssessment(would_cross=True, indeterminate=True)

    now = datetime.now(UTC)
    if candidate.side == OrderSide.ASK:
        opposite_side = OrderSide.BID
        price_filter = OrderBookOrder.price_per_mt_usd >= candidate.price_per_mt_usd
        price_order = OrderBookOrder.price_per_mt_usd.desc()
    else:
        opposite_side = OrderSide.ASK
        price_filter = OrderBookOrder.price_per_mt_usd <= candidate.price_per_mt_usd
        price_order = OrderBookOrder.price_per_mt_usd.asc()
    filters = (
        OrderBookOrder.side == opposite_side,
        OrderBookOrder.product_id == candidate.product_id,
        OrderBookOrder.delivery_point_id == candidate.delivery_point_id,
        OrderBookOrder.availability_window == candidate.availability_window,
        OrderBookOrder.status.in_((OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)),
        OrderBookOrder.organization_id != candidate.organization_id,
        price_filter,
        or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > now),
        OrderBookOrder.provenance == OrganizationProvenance.REAL,
        OrderBookOrder.organization.has(Organization.verification_status == "APPROVED"),
    )
    preview = (
        await db.execute(
            select(OrderBookOrder.id)
            .where(*filters)
            .order_by(price_order, OrderBookOrder.created_at.asc())
            .limit(MAX_CROSSING_ORDERS_PER_MATCH + 1)
        )
    ).scalars().all()
    if len(preview) > MAX_CROSSING_ORDERS_PER_MATCH:
        return PostOnlyAssessment(would_cross=True, indeterminate=True)
    # Row locks are UUID-sorted to preserve the global market lock order. The
    # evaluation is sorted back to price-time priority afterwards.
    crossing_orders = []
    if preview:
        crossing_orders = list(
            (
                await db.execute(
                    select(OrderBookOrder)
                    .where(OrderBookOrder.id.in_(sorted(preview, key=str)), *filters)
                    .order_by(OrderBookOrder.id)
                    .with_for_update()
                    .execution_options(populate_existing=True)
                )
            ).scalars().all()
        )
    owner_ids = sorted(
        {order.owner_user_id for order in crossing_orders if order.owner_user_id},
        key=str,
    )
    organization_ids = sorted(
        {organization.id, *(order.organization_id for order in crossing_orders)}, key=str
    )
    owners = {
        row.id: row
        for row in (
            await db.execute(
                select(User)
                .where(User.id.in_(owner_ids))
                .order_by(User.id)
                .with_for_update()
                .execution_options(populate_existing=True)
            )
        ).scalars().all()
    }
    organizations = {
        row.id: row
        for row in (
            await db.execute(
                select(Organization)
                .where(Organization.id.in_(organization_ids))
                .order_by(Organization.id)
                .with_for_update()
                .execution_options(populate_existing=True)
            )
        ).scalars().all()
    }
    crossing_orders.sort(
        key=lambda order: (
            -order.price_per_mt_usd if candidate.side == OrderSide.ASK else order.price_per_mt_usd,
            order.created_at,
        )
    )
    for crossing in crossing_orders:
        if crossing.owner_user_id is None or not orders_execution_compatible(candidate, crossing):
            continue
        if await order_owner_is_execution_eligible(
            db,
            order=crossing,
            user=owners.get(crossing.owner_user_id),
            organization=organizations.get(crossing.organization_id),
        ):
            return PostOnlyAssessment(
                would_cross=True,
                best_executable_price=crossing.price_per_mt_usd,
            )
    return PostOnlyAssessment(would_cross=False)

"""Transfer an assisted listing to direct customer management on owner action."""
from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.services.audit_actions import ORDER_CANCELLED, ORDER_UPDATED


@dataclass(frozen=True)
class CustomerManagementHandoff:
    order_id: UUID
    previous_support_version: int
    current_support_version: int
    reason_code: str


async def adopt_assisted_order_for_customer_action(
    db: AsyncSession,
    *,
    action: str,
    resource_type: str,
    resource_id: str | None,
    actor_user_id: UUID | None,
) -> CustomerManagementHandoff | None:
    """Make the customer owner authoritative after an ordinary edit/cancel.

    The ordinary order routes already hold the order row and validate exact
    ownership. This sidecar transition happens in the same transaction when
    their canonical audit event is recorded. System/matching/admin actions do
    not satisfy the concrete owner predicate and therefore cannot adopt.
    """
    if (
        action not in {ORDER_UPDATED, ORDER_CANCELLED}
        or resource_type != "order"
        or resource_id is None
        or actor_user_id is None
    ):
        return None
    try:
        order_id = UUID(str(resource_id))
    except ValueError:
        return None

    # Lazy imports avoid making the shared audit module an eager participant in
    # the orderbook/model import graph.
    from app.models.market_support import (
        OrderSupportAttribution,
        SupportManagementAuthority,
    )
    from app.models.orderbook import OrderBookOrder

    row = (
        await db.execute(
            select(OrderBookOrder, OrderSupportAttribution)
            .join(
                OrderSupportAttribution,
                OrderSupportAttribution.order_id == OrderBookOrder.id,
            )
            .where(
                OrderBookOrder.id == order_id,
                OrderBookOrder.owner_user_id == actor_user_id,
                OrderSupportAttribution.management_authority
                == SupportManagementAuthority.SUPPORT_MANDATE,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).one_or_none()
    if row is None:
        return None

    _order, attribution = row
    previous_version = attribution.support_version
    now = datetime.now(UTC)
    reason_code = (
        "CUSTOMER_DIRECT_CANCEL"
        if action == ORDER_CANCELLED
        else "CUSTOMER_DIRECT_EDIT"
    )
    attribution.management_authority = SupportManagementAuthority.CUSTOMER_DIRECT
    attribution.customer_adopted_at = now
    attribution.customer_adopted_by_user_id = actor_user_id
    attribution.support_version += 1
    attribution.last_action_actor_user_id = actor_user_id
    attribution.last_action_reason_code = reason_code
    attribution.updated_at = now
    return CustomerManagementHandoff(
        order_id=order_id,
        previous_support_version=previous_version,
        current_support_version=attribution.support_version,
        reason_code=reason_code,
    )

"""Subscription tier gating middleware.

Usage:
    from app.middleware.subscription import require_tier
    from app.models.subscription import SubscriptionTier

    @router.get("/premium-feature")
    async def premium(
        sub: Subscription = Depends(require_tier(SubscriptionTier.STANDARD)),
        ...
    ):
        ...
"""
from typing import Annotated
from uuid import UUID

from fastapi import Depends, HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.subscription import Subscription, SubscriptionTier
from app.routers.auth_simple import get_current_user

# Tier ordering — higher number = higher privilege
TIER_ORDER: dict[SubscriptionTier, int] = {
    SubscriptionTier.FREE: 0,
    SubscriptionTier.STANDARD: 1,
    SubscriptionTier.ENTERPRISE: 2,
}


async def get_or_create_subscription(db: AsyncSession, org_id: UUID) -> Subscription:
    """Return the Subscription for org_id, creating a free-tier one if none exists."""
    stmt = select(Subscription).where(Subscription.org_id == org_id)
    result = await db.execute(stmt)
    sub = result.scalar_one_or_none()
    if sub is not None:
        return sub

    sub = Subscription(org_id=org_id, tier=SubscriptionTier.FREE)
    db.add(sub)
    await db.commit()
    await db.refresh(sub)
    return sub


def require_tier(minimum_tier: SubscriptionTier):
    """Dependency factory — enforces a minimum subscription tier.

    Returns the Subscription on success, raises HTTP 403 on failure.

    Usage:
        sub = Depends(require_tier(SubscriptionTier.STANDARD))
    """
    async def tier_checker(
        current_user: Annotated[object, Depends(get_current_user)],
        db: Annotated[AsyncSession, Depends(get_db)],
    ) -> Subscription:
        org_id = current_user.organization_id
        sub = await get_or_create_subscription(db, org_id)

        if TIER_ORDER[sub.tier] < TIER_ORDER[minimum_tier]:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=(
                    f"This feature requires a {minimum_tier.value} subscription. "
                    f"Current tier: {sub.tier.value}. "
                    f"Contact sales to upgrade."
                ),
            )
        return sub

    return tier_checker

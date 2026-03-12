"""Subscription management endpoints."""
import uuid
from typing import Annotated, List

from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.middleware.rbac import require_role
from app.middleware.subscription import get_or_create_subscription
from app.models.subscription import Subscription, SubscriptionTier
from app.models.user import User, UserRole
from app.routers.auth_simple import get_current_user
from app.schemas.subscription import SubscriptionResponse, SubscriptionUpdate

router = APIRouter(tags=["subscriptions"])


# ---------------------------------------------------------------------------
# Current-user endpoint
# ---------------------------------------------------------------------------

@router.get("/subscriptions/me", response_model=SubscriptionResponse)
async def get_my_subscription(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> Subscription:
    """Return the calling user's organisation subscription (creates free tier if absent)."""
    return await get_or_create_subscription(db, current_user.organization_id)


# ---------------------------------------------------------------------------
# Admin endpoints
# ---------------------------------------------------------------------------

@router.get("/admin/subscriptions", response_model=List[SubscriptionResponse])
async def list_subscriptions(
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> List[Subscription]:
    """List all subscriptions (admin only)."""
    result = await db.execute(select(Subscription))
    return result.scalars().all()


@router.get("/admin/subscriptions/{org_id}", response_model=SubscriptionResponse)
async def get_subscription_by_org(
    org_id: uuid.UUID,
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> Subscription:
    """Get a single organisation's subscription (admin only)."""
    result = await db.execute(
        select(Subscription).where(Subscription.org_id == org_id)
    )
    sub = result.scalar_one_or_none()
    if sub is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"No subscription found for organisation {org_id}",
        )
    return sub


@router.put("/admin/subscriptions/{org_id}", response_model=SubscriptionResponse)
async def update_subscription(
    org_id: uuid.UUID,
    body: SubscriptionUpdate,
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> Subscription:
    """Create or update an organisation's subscription tier (admin only)."""
    result = await db.execute(
        select(Subscription).where(Subscription.org_id == org_id)
    )
    sub = result.scalar_one_or_none()

    if sub is None:
        sub = Subscription(org_id=org_id, tier=body.tier)
        db.add(sub)
    else:
        sub.tier = body.tier

    await db.commit()
    await db.refresh(sub)
    return sub

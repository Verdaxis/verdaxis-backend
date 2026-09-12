"""Seller-paid per-tonne fee policy for new trades."""
from datetime import UTC, datetime
from decimal import Decimal
from uuid import UUID

from fastapi import HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import Settings, settings
from app.models.subscription import Subscription, SubscriptionTier


def configured_fee_for_tier(
    tier: SubscriptionTier,
    *,
    negotiated_rate: Decimal | None = None,
    configuration: Settings = settings,
) -> Decimal:
    if tier == SubscriptionTier.FREE:
        return configuration.SELLER_FEE_PILOT_PER_MT_USD
    if tier == SubscriptionTier.STANDARD:
        return configuration.SELLER_FEE_PROFESSIONAL_PER_MT_USD
    if negotiated_rate is not None:
        return negotiated_rate
    raise HTTPException(
        status_code=status.HTTP_409_CONFLICT,
        detail="Seller transaction fee is not configured; contact Verdaxis support",
    )


async def resolve_seller_trade_fee(
    db: AsyncSession,
    seller_org_id: UUID,
    *,
    now: datetime | None = None,
    configuration: Settings = settings,
) -> tuple[SubscriptionTier, Decimal]:
    """Resolve the seller's current plan without creating or committing rows."""
    result = await db.execute(
        select(Subscription).where(Subscription.org_id == seller_org_id)
    )
    subscription = result.scalar_one_or_none()
    current_time = now or datetime.now(UTC)
    if (
        subscription is None
        or not subscription.is_active
        or (
            subscription.expires_at is not None
            and subscription.expires_at <= current_time
        )
    ):
        return (
            SubscriptionTier.FREE,
            configured_fee_for_tier(
                SubscriptionTier.FREE, configuration=configuration
            ),
        )

    try:
        tier = SubscriptionTier(subscription.tier)
    except ValueError as error:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Seller subscription tier is invalid; contact Verdaxis support",
        ) from error
    return (
        tier,
        configured_fee_for_tier(
            tier,
            negotiated_rate=subscription.seller_fee_per_mt_usd,
            configuration=configuration,
        ),
    )


def public_fee_schedule(configuration: Settings = settings) -> dict:
    return {
        "currency": "USD",
        "buyer_fee_per_mt_usd": Decimal("0"),
        "seller_fee_per_mt_usd": {
            SubscriptionTier.FREE: configuration.SELLER_FEE_PILOT_PER_MT_USD,
            SubscriptionTier.STANDARD: configuration.SELLER_FEE_PROFESSIONAL_PER_MT_USD,
            SubscriptionTier.ENTERPRISE: None,
        },
    }

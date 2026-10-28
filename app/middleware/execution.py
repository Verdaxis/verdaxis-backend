"""Central gates for authenticated market mutations.

Each gate states which KYC condition its routes add to account admission:

- ``require_execution_eligible_user``: new economic commitments (post or
  amend an order, publish inventory, take or confirm a trade). The KYC
  execution rule applies and a KYC block returns the ``KYC_REQUIRED`` error.
- ``require_market_cleanup_user``: settling an existing commitment (deliver,
  pay). KYC never blocks it.
- ``require_market_admitted_user``: non-executable surfaces (RFQ,
  negotiation, inventory records, suggestions). Unchanged advisory KYC.
"""

from typing import Annotated

from fastapi import Depends, HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.user import Organization, User, UserRole
from app.routers.auth_simple import get_current_user
from app.services.action_policy import ReasonCode
from app.services.execution_policy import (
    MarketPartyPolicy,
    account_is_admitted,
    advisory_kyc_allows,
    user_account_denial,
)
from app.services.kyc_execution import evaluate_kyc_execution, kyc_eligibility_error


async def require_market_party(
    current_user: User,
    db: AsyncSession,
    *,
    policy: MarketPartyPolicy,
) -> User:
    """Admit a buyer or supplier under one market-party policy."""
    if user_account_denial(current_user) == ReasonCode.ADMIN_CANNOT_EXECUTE:
        # There is no established acting-as contract. Admin identity must not
        # be inferred as a buyer or supplier from endpoint fall-through.
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Administrators cannot execute market actions",
        )

    result = await db.execute(
        select(Organization).where(
            Organization.id == current_user.organization_id
        )
    )
    organization = result.scalar_one_or_none()
    if not account_is_admitted(current_user, organization) or (
        policy == MarketPartyPolicy.INDICATIVE and not advisory_kyc_allows(current_user)
    ):
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Execution access is not available")
    if policy == MarketPartyPolicy.EXECUTION:
        # Admission holds, so a refusal here is the KYC rule: say so.
        eligibility = await evaluate_kyc_execution(db, current_user)
        if not eligibility.eligible:
            raise kyc_eligibility_error(eligibility)
    return current_user


async def require_execution_eligible_user(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> User:
    """Require account admission and the KYC execution rule."""
    return await require_market_party(
        current_user, db, policy=MarketPartyPolicy.EXECUTION
    )


async def require_market_cleanup_user(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> User:
    """Require account admission only; KYC never blocks cleanup."""
    return await require_market_party(
        current_user, db, policy=MarketPartyPolicy.CLEANUP
    )


async def require_market_admitted_user(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> User:
    """Require account admission with the unchanged advisory KYC."""
    return await require_market_party(
        current_user, db, policy=MarketPartyPolicy.INDICATIVE
    )

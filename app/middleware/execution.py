"""Central gate for mutations that can create or change market state."""

from typing import Annotated

from fastapi import Depends, HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.user import Organization, User, UserRole, UserStatus
from app.routers.auth_simple import get_current_user
from app.services.execution_policy import execution_party_is_eligible


async def require_execution_eligible_user(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> User:
    """Require a fully admitted buyer/supplier for executable mutations."""
    if current_user.role == UserRole.ADMIN:
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
    if not await execution_party_is_eligible(db, user=current_user, organization=organization):
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Execution access is not available")
    return current_user

"""Admission for indicative RFQs, negotiations and supplier offers.

These routes do not execute trades. Retain their existing reject-only KYC
policy independently of the stricter execution gate.
"""
from typing import Annotated

from fastapi import Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.user import User
from app.routers.auth_simple import get_current_user
from app.services.execution_policy import market_party_is_admitted


async def require_security_market_admission(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> User:
    if not await market_party_is_admitted(db, user=current_user):
        raise HTTPException(403, detail="Execution access is not available")
    return current_user

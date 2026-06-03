"""Internal monitor support endpoints."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Header, HTTPException, status
from pydantic import BaseModel, EmailStr
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import settings
from app.database import get_db
from app.models.user import Organization, User
from app.services.monitor_canary import get_monitor_canary_domain

router = APIRouter(prefix="/monitor", tags=["monitor"], include_in_schema=False)


class CanaryCleanupRequest(BaseModel):
    email: EmailStr


class CanaryCleanupResponse(BaseModel):
    deleted_users: int
    deleted_orgs: int


def _require_monitor_token(x_monitor_token: str | None = Header(default=None)) -> None:
    if not settings.MONITOR_TOKEN:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Not found")
    if not x_monitor_token or x_monitor_token != settings.MONITOR_TOKEN:
        raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="Unauthorized")


def _assert_canary_email(email: str) -> str:
    domain = get_monitor_canary_domain(email)
    if domain is None:
        raise HTTPException(
            status_code=422,
            detail="Only monitor canary emails can be cleaned up",
        )
    return domain


@router.post("/signup-canary-cleanup", status_code=status.HTTP_200_OK, response_model=CanaryCleanupResponse)
async def cleanup_signup_canary(
    payload: CanaryCleanupRequest,
    _: None = Depends(_require_monitor_token),
    db: AsyncSession = Depends(get_db),
) -> CanaryCleanupResponse:
    """Remove the synthetic user/org created by the external signup canary."""
    domain = _assert_canary_email(str(payload.email))
    user = (
        await db.execute(select(User).where(User.email == str(payload.email)))
    ).scalar_one_or_none()

    deleted_users = 0
    deleted_orgs = 0
    organization_id = user.organization_id if user else None
    if user:
        await db.delete(user)
        deleted_users = 1
        await db.flush()

    org = None
    if organization_id:
        org = (
            await db.execute(select(Organization).where(Organization.id == organization_id))
        ).scalar_one_or_none()
    if org is None:
        org = (
            await db.execute(select(Organization).where(Organization.domain == domain))
        ).scalar_one_or_none()

    if org and org.domain == domain:
        member_count = (
            await db.execute(select(func.count()).where(User.organization_id == org.id))
        ).scalar() or 0
        if member_count == 0:
            await db.delete(org)
            deleted_orgs = 1

    await db.commit()
    return CanaryCleanupResponse(deleted_users=deleted_users, deleted_orgs=deleted_orgs)

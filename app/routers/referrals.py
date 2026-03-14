"""Referral link endpoints — generate, share, track, leaderboard."""
import re
from datetime import datetime, UTC
from typing import Annotated

from fastapi import APIRouter, Depends, HTTPException, Request as _Request
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.referral import Referral, ReferralStatus, generate_referral_code
from app.models.user import User
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.config import settings
from app.schemas.referral import (
    ReferralCodeResponse,
    ReferralInviteRequest,
    ReferralListItem,
    ReferralStatsResponse,
    LeaderboardEntry,
    ResolveCodeResponse,
)

router = APIRouter(prefix="/referrals", tags=["referrals"])

_CODE_PATTERN = re.compile(r"^VDX-[A-Z0-9]{6}$")


def _validate_code_format(code: str) -> bool:
    """Check if a referral code matches the expected format."""
    return bool(_CODE_PATTERN.match(code))


@router.get("/my-code", response_model=ReferralCodeResponse)
async def get_my_referral_code(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Get the current user's referral code. Generates one if missing."""
    if not current_user.email_verified:
        raise HTTPException(status_code=403, detail="Email must be verified to get a referral code")

    if not current_user.referral_code:
        for _ in range(10):
            code = generate_referral_code()
            existing = await db.execute(
                select(User.id).where(User.referral_code == code)
            )
            if not existing.scalar_one_or_none():
                current_user.referral_code = code
                await db.commit()
                break
        else:
            raise HTTPException(status_code=500, detail="Failed to generate unique code")

    return ReferralCodeResponse(
        referral_code=current_user.referral_code,
        referral_link=f"{settings.FRONTEND_URL}/invite/{current_user.referral_code}",
    )


@router.get("/my-referrals", response_model=ReferralStatsResponse)
async def get_my_referrals(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """List all users referred by the current user."""
    stmt = (
        select(Referral)
        .where(Referral.referrer_id == current_user.id)
        .order_by(Referral.created_at.desc())
    )
    result = await db.execute(stmt)
    referrals = result.scalars().all()

    items = []
    for ref in referrals:
        user_stmt = (
            select(User)
            .options(selectinload(User.organization))
            .where(User.id == ref.referred_user_id)
        )
        user_result = await db.execute(user_stmt)
        referred_user = user_result.scalar_one_or_none()

        items.append(ReferralListItem(
            organization_name=referred_user.organization.name if referred_user and referred_user.organization else None,
            role=referred_user.role.value if referred_user and referred_user.role else None,
            status=ref.status.value,
            signed_up_at=ref.created_at,
        ))

    total = len(referrals)
    verified = sum(1 for r in referrals if r.status in (ReferralStatus.VERIFIED, ReferralStatus.ACTIVE))
    active = sum(1 for r in referrals if r.status == ReferralStatus.ACTIVE)

    return ReferralStatsResponse(
        total=total, verified=verified, active=active, referrals=items,
    )


@router.get("/leaderboard", response_model=list[LeaderboardEntry])
async def get_leaderboard(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Top 10 referrers platform-wide."""
    stmt = (
        select(
            Referral.referrer_id,
            func.count(Referral.id).label("ref_count"),
        )
        .group_by(Referral.referrer_id)
        .order_by(func.count(Referral.id).desc())
        .limit(10)
    )
    result = await db.execute(stmt)
    rows = result.all()

    entries = []
    for rank, (referrer_id, count) in enumerate(rows, 1):
        user_stmt = (
            select(User)
            .options(selectinload(User.organization))
            .where(User.id == referrer_id)
        )
        user_result = await db.execute(user_stmt)
        user = user_result.scalar_one_or_none()
        if not user:
            continue

        entries.append(LeaderboardEntry(
            rank=rank,
            user_name=f"{user.first_name or ''} {user.last_name or ''}".strip() or "Anonymous",
            organization_name=user.organization.name if user.organization else None,
            referral_count=count,
        ))

    return entries


@router.post("/invite")
@limiter.limit("10/hour")
async def send_referral_invite(
    request: _Request,
    body: ReferralInviteRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Send a referral invite email."""
    if not current_user.email_verified:
        raise HTTPException(status_code=403, detail="Email must be verified to send invites")

    existing = await db.execute(
        select(User.id).where(User.email == body.email)
    )
    if existing.scalar_one_or_none():
        raise HTTPException(status_code=400, detail="This email is already registered on Verdaxis")

    if body.email == current_user.email:
        raise HTTPException(status_code=400, detail="Cannot invite yourself")

    if not current_user.referral_code:
        current_user.referral_code = generate_referral_code()
        await db.commit()

    from app.services.email import send_referral_invite_email
    sent = await send_referral_invite_email(
        to_email=body.email,
        referrer_name=f"{current_user.first_name or ''} {current_user.last_name or ''}".strip(),
        referral_code=current_user.referral_code,
    )

    if not sent:
        raise HTTPException(status_code=502, detail="Failed to send invite email")

    return {"message": "Invitation sent", "email": body.email}


@router.get("/resolve/{code}", response_model=ResolveCodeResponse)
async def resolve_referral_code(
    code: str,
    db: AsyncSession = Depends(get_db),
):
    """Public endpoint — validate a referral code and return referrer info."""
    if not _validate_code_format(code):
        return ResolveCodeResponse(valid=False)

    stmt = (
        select(User)
        .options(selectinload(User.organization))
        .where(User.referral_code == code)
    )
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user:
        return ResolveCodeResponse(valid=False)

    return ResolveCodeResponse(
        valid=True,
        organization_name=user.organization.name if user.organization else None,
        organization_type=user.organization.type.value if user.organization and user.organization.type else None,
        referrer_name=f"{user.first_name or ''} {user.last_name or ''}".strip() or None,
    )

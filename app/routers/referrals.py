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
from app.services.email import send_referral_invite_email
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


async def _assign_unique_referral_code(db: AsyncSession, user: User, retries: int = 10) -> str:
    """Generate and assign a collision-free referral code to a user."""
    for _ in range(retries):
        code = generate_referral_code()
        existing = await db.execute(
            select(User.id).where(User.referral_code == code)
        )
        if not existing.scalar_one_or_none():
            user.referral_code = code
            return code
    raise HTTPException(status_code=500, detail="Failed to generate unique referral code")


def _display_name(user: User) -> str:
    """Format a user's display name with a safe fallback."""
    return f"{user.first_name or ''} {user.last_name or ''}".strip() or "Anonymous"


@router.get("/my-code", response_model=ReferralCodeResponse)
async def get_my_referral_code(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Get the current user's referral code. Generates one if missing."""
    if not current_user.email_verified:
        raise HTTPException(status_code=403, detail="Email must be verified to get a referral code")

    if not current_user.referral_code:
        await _assign_unique_referral_code(db, current_user)
        await db.commit()

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
        .options(
            selectinload(Referral.referred_user).selectinload(User.organization)
        )
        .order_by(Referral.created_at.desc())
    )
    result = await db.execute(stmt)
    referrals = result.scalars().all()

    items = [
        ReferralListItem(
            organization_name=(
                ref.referred_user.organization.name
                if ref.referred_user and ref.referred_user.organization
                else None
            ),
            role=(
                ref.referred_user.role.value
                if ref.referred_user and ref.referred_user.role
                else None
            ),
            status=ref.status.value,
            signed_up_at=ref.created_at,
        )
        for ref in referrals
    ]

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
    # Step 1: aggregate counts
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

    if not rows:
        return []

    # Step 2: batch-fetch all 10 users in one query (fixes N+1)
    referrer_ids = [row[0] for row in rows]
    counts_by_id = {row[0]: row[1] for row in rows}

    users_stmt = (
        select(User)
        .options(selectinload(User.organization))
        .where(User.id.in_(referrer_ids))
    )
    users_result = await db.execute(users_stmt)
    users_by_id = {u.id: u for u in users_result.scalars()}

    # Step 3: build ranked entries preserving count order
    entries = []
    rank = 1
    for referrer_id, count in rows:
        user = users_by_id.get(referrer_id)
        if not user:
            continue
        entries.append(LeaderboardEntry(
            rank=rank,
            user_name=_display_name(user),
            organization_name=user.organization.name if user.organization else None,
            referral_count=count,
        ))
        rank += 1

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

    if body.email == current_user.email:
        raise HTTPException(status_code=400, detail="Cannot invite yourself")

    existing = await db.execute(
        select(User.id).where(User.email == body.email)
    )
    if existing.scalar_one_or_none():
        # Generic message to prevent email enumeration (I-4)
        return {"message": "Invitation processed", "email": body.email}

    if not current_user.referral_code:
        await _assign_unique_referral_code(db, current_user)
        await db.commit()

    sent = await send_referral_invite_email(
        to_email=body.email,
        referrer_name=_display_name(current_user),
        referral_code=current_user.referral_code,
    )

    if not sent:
        raise HTTPException(status_code=502, detail="Failed to send invite email")

    return {"message": "Invitation processed", "email": body.email}


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

    name = _display_name(user)
    return ResolveCodeResponse(
        valid=True,
        organization_name=user.organization.name if user.organization else None,
        organization_type=user.organization.type.value if user.organization and user.organization.type else None,
        referrer_name=name if name != "Anonymous" else None,
    )

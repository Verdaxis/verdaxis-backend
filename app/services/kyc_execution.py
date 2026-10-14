"""Shared KYC admission rule and its SQL projection."""
from datetime import datetime, timezone
from dataclasses import dataclass

from fastapi import HTTPException
from sqlalchemy import and_, or_, select, func
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.kyc_execution_grace import KYCExecutionGrace
from app.models.user import User


@dataclass(frozen=True)
class KYCEligibility:
    eligible: bool
    reason: str
    grace_expires_at: datetime | None = None


def kyc_execution_clause(user=User, *, now=None):
    """Use the same current-membership and strict expiry rule in SQL reads."""
    grace = select(KYCExecutionGrace.id).where(
        KYCExecutionGrace.user_id == user.id,
        KYCExecutionGrace.organization_id == user.organization_id,
        KYCExecutionGrace.expires_at > (now if now is not None else func.now()),
    ).exists()
    return and_(
        user.kyc_status != "REJECTED",
        or_(and_(user.kyc_status == "APPROVED", user.kyc_organization_id == user.organization_id), grace),
    )


async def kyc_execution_eligibility(
    db: AsyncSession, user: User, *, now: datetime | None = None,
) -> KYCEligibility:
    status = getattr(user, "kyc_status", None)
    organization_id = getattr(user, "organization_id", None)
    if status == "APPROVED" and organization_id is not None and user.kyc_organization_id == organization_id:
        return KYCEligibility(True, "KYC_APPROVED")
    grace = None
    if organization_id is not None and hasattr(db, "execute"):
        grace = (await db.execute(select(KYCExecutionGrace).where(
            KYCExecutionGrace.user_id == user.id,
            KYCExecutionGrace.organization_id == organization_id,
        ))).scalar_one_or_none()
    expires_at = grace.expires_at if grace is not None else None
    # SQLite drops timezone metadata; persisted timestamps are always UTC.
    if expires_at is not None and expires_at.tzinfo is None:
        expires_at = expires_at.replace(tzinfo=timezone.utc)
    if status == "REJECTED":
        return KYCEligibility(False, "KYC_REJECTED", expires_at)
    if expires_at is not None and (now or datetime.now(timezone.utc)) < expires_at:
        return KYCEligibility(True, "KYC_GRACE", expires_at)
    return KYCEligibility(False, "KYC_REQUIRED", expires_at)


async def require_kyc_execution(db: AsyncSession, user: User) -> None:
    eligibility = await kyc_execution_eligibility(db, user)
    if not eligibility.eligible:
        raise HTTPException(403, detail={
            "code": "KYC_REQUIRED",
            "message": "Complete KYC verification to trade.",
            "kyc_status": getattr(user, "kyc_status", "PENDING"),
            "grace_expires_at": eligibility.grace_expires_at.isoformat() if eligibility.grace_expires_at else None,
        })

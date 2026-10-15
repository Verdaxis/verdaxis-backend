"""Shared KYC admission rule and its SQL projection.

The rule is behind one switch, ``KYC_EXECUTION_GATE_ENABLED``. When the gate is
off (Private Beta) this whole module follows the pre-gate advisory rule that
the INDICATIVE policy keeps: only a rejection or a review bound to another
organization revokes access, and an eligible user reports ``KYC_NOT_ENFORCED``.
The switch is read here, in the one decision point, so routes and SQL mirrors
never carry their own gate checks.
"""
from dataclasses import dataclass
from datetime import datetime, timezone

from fastapi import HTTPException
from sqlalchemy import and_, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import settings
from app.models.kyc_execution_grace import KYCExecutionGrace
from app.models.user import User


@dataclass(frozen=True)
class KYCEligibility:
    eligible: bool
    reason: str
    grace_expires_at: datetime | None = None


def kyc_execution_gate_enabled() -> bool:
    """Read the one switch. Enabled is the fail-safe default."""
    return bool(settings.KYC_EXECUTION_GATE_ENABLED)


def kyc_execution_gate_startup_message() -> str:
    """The one line logged at startup so operators see the gate state."""
    state = "enabled" if kyc_execution_gate_enabled() else "disabled"
    return f"KYC execution gate: {state}"


def advisory_kyc_allows(user: User) -> bool:
    """The pre-gate advisory rule kept for indicative surfaces.

    Only a rejection or a review bound to another organization revokes
    access; every other status is advisory.
    """
    if getattr(user, "kyc_status", None) == "REJECTED":
        return False
    kyc_organization_id = getattr(user, "kyc_organization_id", None)
    return (
        kyc_organization_id is None
        or kyc_organization_id == user.organization_id
    )


def advisory_kyc_clause(user=User):
    """SQL mirror of ``advisory_kyc_allows`` for the pre-gate rule."""
    return and_(
        user.kyc_status != "REJECTED",
        or_(
            user.kyc_organization_id.is_(None),
            user.kyc_organization_id == user.organization_id,
        ),
    )


def kyc_execution_clause(user=User, *, now=None):
    """Use the same current-membership and strict expiry rule in SQL reads.

    With the gate off this returns the advisory mirror, so read surfaces and
    matching follow the same switch as the Python decision.
    """
    if not kyc_execution_gate_enabled():
        return advisory_kyc_clause(user)
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
    if not kyc_execution_gate_enabled():
        # Gate off: the pre-gate advisory rule. A rejection or a review bound
        # to another organization keeps its blocking reason; nothing else
        # blocks, so grace and the review status no longer matter and no
        # grace deadline is reported.
        if advisory_kyc_allows(user):
            return KYCEligibility(True, "KYC_NOT_ENFORCED")
        return KYCEligibility(
            False, "KYC_REJECTED" if status == "REJECTED" else "KYC_REQUIRED"
        )
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

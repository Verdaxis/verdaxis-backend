"""The KYC execution rule shared by route gates, matching, and read mirrors.

A user may create a new economic commitment only when the user's KYC is
APPROVED for the user's current organization, or the user holds a valid grace
row for that organization. REJECTED always blocks and overrides grace. Every
other status (PENDING, SUBMITTED, legacy REVIEW_REQUIRED) is not approved.

Assisted (market support) execution acts for a customer organization, so the
same rule applies to the organization: at least one admitted member must be
eligible. The administrator's own KYC is irrelevant.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from enum import Enum
from uuid import UUID

from fastapi import HTTPException, status
from sqlalchemy import and_, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import aliased

from app.models.kyc_execution_grace import KycExecutionGrace
from app.models.user import User, UserRole, UserStatus

KYC_APPROVED = "APPROVED"
KYC_REJECTED = "REJECTED"
# The column default; also used for a transient user that was never flushed.
KYC_DEFAULT_STATUS = "PENDING"

KYC_REQUIRED_CODE = "KYC_REQUIRED"
KYC_REQUIRED_MESSAGE = "Complete KYC verification to trade."

_EXECUTING_ROLES = (UserRole.BUYER, UserRole.SUPPLIER)


class ExecutionEligibilityReason(str, Enum):
    KYC_APPROVED = "KYC_APPROVED"
    KYC_GRACE = "KYC_GRACE"
    KYC_REQUIRED = "KYC_REQUIRED"
    KYC_REJECTED = "KYC_REJECTED"
    # Administrators and users without an organization never trade as a party.
    NOT_APPLICABLE = "NOT_APPLICABLE"


_ELIGIBLE_REASONS = frozenset(
    {ExecutionEligibilityReason.KYC_APPROVED, ExecutionEligibilityReason.KYC_GRACE}
)


@dataclass(frozen=True)
class KycExecutionEligibility:
    """The KYC part of execution eligibility; account admission is separate."""

    reason: ExecutionEligibilityReason
    kyc_status: str
    # The deadline that applies to the user: a future deadline while grace is
    # valid, or the passed deadline once it has expired. None otherwise.
    grace_expires_at: datetime | None = None

    @property
    def eligible(self) -> bool:
        return self.reason in _ELIGIBLE_REASONS


def _as_utc(value: datetime) -> datetime:
    """SQLite returns naive values for timezone-aware columns; they are UTC."""
    if value.tzinfo is None or value.utcoffset() is None:
        return value.replace(tzinfo=UTC)
    return value.astimezone(UTC)


async def _grace_expiry(
    db: AsyncSession, *, user_id: UUID, organization_id: UUID
) -> datetime | None:
    expires_at = (
        await db.execute(
            select(KycExecutionGrace.expires_at).where(
                KycExecutionGrace.user_id == user_id,
                KycExecutionGrace.organization_id == organization_id,
            )
        )
    ).scalar_one_or_none()
    return _as_utc(expires_at) if expires_at is not None else None


async def evaluate_kyc_execution(
    db: AsyncSession, user: User, *, now: datetime | None = None
) -> KycExecutionEligibility:
    """Apply the KYC execution rule to one user and the user's current org."""
    kyc_status = getattr(user, "kyc_status", None) or KYC_DEFAULT_STATUS
    organization_id = getattr(user, "organization_id", None)
    if getattr(user, "role", None) not in _EXECUTING_ROLES or organization_id is None:
        return KycExecutionEligibility(ExecutionEligibilityReason.NOT_APPLICABLE, kyc_status)
    if kyc_status == KYC_REJECTED:
        return KycExecutionEligibility(ExecutionEligibilityReason.KYC_REJECTED, kyc_status)
    if (
        kyc_status == KYC_APPROVED
        and getattr(user, "kyc_organization_id", None) == organization_id
    ):
        return KycExecutionEligibility(ExecutionEligibilityReason.KYC_APPROVED, kyc_status)

    # A grace row is bound to one organization, so a membership change voids it.
    grace_expires_at = await _grace_expiry(
        db, user_id=user.id, organization_id=organization_id
    )
    current_time = _as_utc(now) if now is not None else datetime.now(UTC)
    if grace_expires_at is not None and current_time < grace_expires_at:
        return KycExecutionEligibility(
            ExecutionEligibilityReason.KYC_GRACE, kyc_status, grace_expires_at
        )
    return KycExecutionEligibility(
        ExecutionEligibilityReason.KYC_REQUIRED, kyc_status, grace_expires_at
    )


def kyc_required_error(
    *, kyc_status: str | None, grace_expires_at: datetime | None
) -> HTTPException:
    """The contract error for a blocked execution."""
    return HTTPException(
        status_code=status.HTTP_403_FORBIDDEN,
        detail={
            "code": KYC_REQUIRED_CODE,
            "message": KYC_REQUIRED_MESSAGE,
            "kyc_status": kyc_status,
            "grace_expires_at": (
                grace_expires_at.isoformat() if grace_expires_at is not None else None
            ),
        },
    )


def kyc_eligibility_error(eligibility: KycExecutionEligibility) -> HTTPException:
    return kyc_required_error(
        kyc_status=eligibility.kyc_status,
        grace_expires_at=eligibility.grace_expires_at,
    )


def kyc_execution_clause(user, organization_id, *, now=None):
    """SQL mirror of ``evaluate_kyc_execution`` for a user acting for an org.

    ``user`` is the User entity or an alias of it; ``organization_id`` is a
    column or a value. ``now`` defaults to the database clock.
    """
    now_value = now if now is not None else func.now()
    valid_grace = (
        select(KycExecutionGrace.id)
        .where(
            KycExecutionGrace.user_id == user.id,
            KycExecutionGrace.organization_id == organization_id,
            KycExecutionGrace.expires_at > now_value,
        )
        .exists()
    )
    approved_for_organization = and_(
        user.kyc_status == KYC_APPROVED,
        user.kyc_organization_id == organization_id,
    )
    return and_(
        user.organization_id == organization_id,
        user.kyc_status != KYC_REJECTED,
        or_(approved_for_organization, valid_grace),
    )


def customer_organization_eligibility_clause(organization_id, *, now=None):
    """SQL: the organization has at least one admitted, KYC-eligible member."""
    member = aliased(User)
    return (
        select(member.id)
        .where(
            member.organization_id == organization_id,
            member.role.in_(_EXECUTING_ROLES),
            member.status == UserStatus.APPROVED,
            member.email_verified.is_(True),
            member.must_change_password.is_(False),
            kyc_execution_clause(member, organization_id, now=now),
        )
        .exists()
    )


async def customer_organization_is_execution_eligible(
    db: AsyncSession, organization_id: UUID | None, *, now: datetime | None = None
) -> bool:
    """The assisted-execution rule: one shared helper for every assisted path."""
    if organization_id is None:
        return False
    current_time = _as_utc(now) if now is not None else datetime.now(UTC)
    return bool(
        await db.scalar(
            select(
                customer_organization_eligibility_clause(organization_id, now=current_time)
            )
        )
    )


def customer_organization_kyc_error() -> HTTPException:
    """An assisted denial names no member, so it carries no personal status."""
    return kyc_required_error(kyc_status=None, grace_expires_at=None)

"""Authoritative onboarding-state classification for operator attention."""

from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from enum import StrEnum

from sqlalchemy import String, and_, cast, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.audit import AuditLog
from app.models.product_analytics import UserStatusTransition
from app.models.registration import OrganizationJoinRequest, PendingRegistration
from app.models.user import Organization, User, UserRole, UserStatus
from app.services.audit_actions import ADMIN_ORGANIZATION_APPROVED
from app.services.monitor_canary import is_monitor_canary_email


FIRST_LOGIN_OVERDUE = timedelta(hours=2)
VERIFICATION_STALLED = timedelta(hours=24)
ORGANIZATION_SETUP_WARNING = timedelta(minutes=5)
EXCLUDED_PROVENANCE = {"DEMO", "TEST", "CANARY"}


class AttentionStage(StrEnum):
    REJECTED = "rejected"
    ORGANIZATION_SETUP_EXPIRING = "organization_setup_expiring"
    VERIFICATION_STALLED = "verification_stalled"
    APPROVAL_REQUIRED = "approval_required"
    FIRST_LOGIN_OVERDUE = "first_login_overdue"


@dataclass(frozen=True)
class OnboardingCandidate:
    key: str
    email: str
    first_name: str | None
    last_name: str | None
    role: str | None
    created_at: datetime
    email_verified: bool
    verification_expires_at: datetime | None
    user_status: str | None
    organization_name: str | None
    organization_status: str | None
    organization_provenance: str | None
    membership_status: str | None
    membership_reviewed_at: datetime | None
    user_approved_at: datetime | None
    organization_approved_at: datetime | None
    last_login: datetime | None
    pending_registration_expires_at: datetime | None


@dataclass(frozen=True)
class Attention:
    candidate: OnboardingCandidate
    stage: AttentionStage
    since: datetime


def _value(value) -> str | None:
    if value is None:
        return None
    return str(getattr(value, "value", value)).upper()


def _latest(*values: datetime | None) -> datetime:
    return max(value for value in values if value is not None)


def classify_candidate(candidate: OnboardingCandidate, now: datetime) -> Attention | None:
    if (
        _value(candidate.role) == "ADMIN"
        or is_monitor_canary_email(candidate.email)
        or _value(candidate.organization_provenance) in EXCLUDED_PROVENANCE
    ):
        return None

    statuses = (
        _value(candidate.user_status),
        _value(candidate.organization_status),
        _value(candidate.membership_status),
    )
    if "REJECTED" in statuses:
        return Attention(candidate, AttentionStage.REJECTED, candidate.created_at)

    pending_expiry = candidate.pending_registration_expires_at
    if pending_expiry is not None:
        if pending_expiry <= now + ORGANIZATION_SETUP_WARNING:
            return Attention(
                candidate,
                AttentionStage.ORGANIZATION_SETUP_EXPIRING,
                pending_expiry,
            )
        return None

    verification_expiry = (
        candidate.verification_expires_at
        or candidate.created_at + VERIFICATION_STALLED
    )
    if not candidate.email_verified:
        if verification_expiry <= now:
            return Attention(
                candidate,
                AttentionStage.VERIFICATION_STALLED,
                verification_expiry,
            )
        return None

    if statuses != ("APPROVED", "APPROVED", "APPROVED"):
        return Attention(
            candidate,
            AttentionStage.APPROVAL_REQUIRED,
            candidate.created_at,
        )

    approved_at = _latest(
        candidate.created_at,
        candidate.user_approved_at,
        candidate.organization_approved_at,
        candidate.membership_reviewed_at,
    )
    if candidate.last_login is None and approved_at <= now - FIRST_LOGIN_OVERDUE:
        return Attention(
            candidate,
            AttentionStage.FIRST_LOGIN_OVERDUE,
            approved_at,
        )
    return None


def _elapsed(since: datetime, now: datetime) -> str:
    seconds = int((now - since).total_seconds())
    if seconds < 0:
        return f"in {max(1, (-seconds + 59) // 60)}m"
    minutes = seconds // 60
    if minutes < 60:
        return f"{minutes}m ago"
    hours = minutes // 60
    if hours < 48:
        return f"{hours}h {minutes % 60}m ago"
    return f"{hours // 24}d ago"


_STAGE_COPY = {
    AttentionStage.REJECTED: ("Rejected", "Review the rejection and contact the user if appropriate."),
    AttentionStage.ORGANIZATION_SETUP_EXPIRING: (
        "Organization setup expiring",
        "Contact the user before their organization setup expires.",
    ),
    AttentionStage.VERIFICATION_STALLED: (
        "Email verification stalled",
        "Ask the user to request a new verification email.",
    ),
    AttentionStage.APPROVAL_REQUIRED: (
        "Approval required",
        "Review and complete the outstanding onboarding approvals.",
    ),
    AttentionStage.FIRST_LOGIN_OVERDUE: (
        "First login overdue",
        "Contact the user and offer the password-reset flow if needed.",
    ),
}


def _display_name(candidate: OnboardingCandidate) -> str:
    return " ".join(
        part.strip() for part in (candidate.first_name or "", candidate.last_name or "") if part.strip()
    ) or candidate.email


def format_attention_message(attention: Attention, now: datetime) -> str:
    candidate = attention.candidate
    label, action = _STAGE_COPY[attention.stage]
    organization = candidate.organization_name or "Not created"
    return (
        "Verdaxis onboarding needs attention\n"
        f"{_display_name(candidate)} · {candidate.email}\n"
        f"Organization: {organization}\n"
        f"Stage: {label}\n"
        f"Timing: {_elapsed(attention.since, now)}\n"
        f"Action: {action}"
    )


def format_recovery_message(candidate: OnboardingCandidate) -> str:
    return (
        "Verdaxis onboarding recovered\n"
        f"{_display_name(candidate)} · {candidate.email}\n"
        f"Organization: {candidate.organization_name or 'Not created'}\n"
        "The previously reported onboarding issue is now resolved."
    )


async def load_candidates(db: AsyncSession) -> list[OnboardingCandidate]:
    user_approved_at = (
        select(func.max(UserStatusTransition.effective_at))
        .where(
            UserStatusTransition.user_id == User.id,
            UserStatusTransition.to_status == UserStatus.APPROVED,
        )
        .correlate(User)
        .scalar_subquery()
    )
    organization_approved_at = (
        select(func.max(AuditLog.timestamp))
        .where(
            AuditLog.action == ADMIN_ORGANIZATION_APPROVED,
            AuditLog.resource_id == cast(Organization.id, String),
        )
        .correlate(Organization)
        .scalar_subquery()
    )
    rows = (
        await db.execute(
            select(
                User,
                Organization,
                OrganizationJoinRequest,
                user_approved_at.label("user_approved_at"),
                organization_approved_at.label("organization_approved_at"),
            )
            .outerjoin(Organization, Organization.id == User.organization_id)
            .outerjoin(
                OrganizationJoinRequest,
                and_(
                    OrganizationJoinRequest.user_id == User.id,
                    OrganizationJoinRequest.organization_id == User.organization_id,
                ),
            )
            .where(User.role != UserRole.ADMIN)
        )
    ).all()

    candidates = [
        OnboardingCandidate(
            key=f"user:{user.id}",
            email=user.email,
            first_name=user.first_name,
            last_name=user.last_name,
            role=_value(user.role),
            created_at=user.created_at,
            email_verified=user.email_verified,
            verification_expires_at=user.email_verification_token_expires_at,
            user_status=_value(user.status),
            organization_name=organization.name if organization else None,
            organization_status=organization.verification_status if organization else None,
            organization_provenance=_value(organization.provenance) if organization else None,
            membership_status=_value(membership.status) if membership else None,
            membership_reviewed_at=membership.reviewed_at if membership else None,
            user_approved_at=approved_at,
            organization_approved_at=org_approved_at,
            last_login=user.last_login,
            pending_registration_expires_at=None,
        )
        for user, organization, membership, approved_at, org_approved_at in rows
    ]

    pending_rows = (
        await db.execute(
            select(PendingRegistration).where(PendingRegistration.used_at.is_(None))
        )
    ).scalars()
    candidates.extend(
        OnboardingCandidate(
            key=f"pending:{pending.id}",
            email=pending.email,
            first_name=pending.first_name,
            last_name=pending.last_name,
            role=_value(pending.role),
            created_at=pending.created_at,
            email_verified=False,
            verification_expires_at=None,
            user_status=None,
            organization_name=None,
            organization_status=None,
            organization_provenance=None,
            membership_status=None,
            membership_reviewed_at=None,
            user_approved_at=None,
            organization_approved_at=None,
            last_login=None,
            pending_registration_expires_at=pending.expires_at,
        )
        for pending in pending_rows
    )
    return candidates

"""Send the one-time KYC grace email, at most once per grace row.

An operator CLI runs this; web workers never send it. Each delivery is
recorded as a ``kyc.grace_notice_sent`` audit row in the same transaction,
and a dedicated transaction lock keeps overlapping runs from sending twice.
"""
from __future__ import annotations

from datetime import datetime
from enum import Enum
from uuid import UUID

from sqlalchemy import select, text
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.audit import AuditLog
from app.models.kyc_execution_grace import KycExecutionGrace
from app.models.user import User, UserStatus
from app.services.audit_actions import KYC_GRACE_NOTICE_SENT
from app.services.audit_service import record_audit
from app.services.email import (
    build_kyc_grace_notice_email_payload,
    send_kyc_grace_notice_email,
)
from app.services.kyc_execution import ExecutionEligibilityReason, evaluate_kyc_execution

# Dedicated transaction lock id; distinct from the other job lock ids.
KYC_GRACE_NOTICE_LOCK_ID = 8246192061
GRACE_NOTICE_RESOURCE_TYPE = "kyc_execution_grace"


class KycGraceNoticeInProgress(RuntimeError):
    """Another run holds the notice lock."""


class NoticeOutcome(str, Enum):
    SENT = "sent"
    # The user no longer depends on grace (approved, rejected, moved, or
    # expired) or cannot receive email. Not recorded, so a later run
    # evaluates the row again.
    SKIPPED = "skipped"
    # The provider did not accept the email; the next run retries it.
    FAILED = "failed"


async def _hold_notice_lock(db: AsyncSession) -> None:
    """Take the run lock for the current transaction (PostgreSQL only)."""
    if db.get_bind().dialect.name != "postgresql":
        return
    acquired = await db.scalar(
        text("SELECT pg_try_advisory_xact_lock(:lock_id)"),
        {"lock_id": KYC_GRACE_NOTICE_LOCK_ID},
    )
    if not acquired:
        raise KycGraceNoticeInProgress("Another KYC grace notice run is active")


def _sent_notice_ids():
    return select(AuditLog.resource_id).where(
        AuditLog.action == KYC_GRACE_NOTICE_SENT,
        AuditLog.resource_type == GRACE_NOTICE_RESOURCE_TYPE,
    )


async def _unsent_grace_ids(db: AsyncSession) -> list[UUID]:
    sent = set((await db.execute(_sent_notice_ids())).scalars().all())
    grace_ids = (
        await db.execute(
            select(KycExecutionGrace.id).order_by(
                KycExecutionGrace.enrolled_at, KycExecutionGrace.id
            )
        )
    ).scalars().all()
    return [grace_id for grace_id in grace_ids if str(grace_id) not in sent]


async def _send_one_notice(
    db: AsyncSession, grace_id: UUID, *, now: datetime | None
) -> NoticeOutcome:
    """Send and record one notice under the run lock; the caller commits."""
    await _hold_notice_lock(db)
    # Re-check under the lock: an overlapping run may have sent it already.
    sent_record = await db.scalar(
        _sent_notice_ids().where(AuditLog.resource_id == str(grace_id)).limit(1)
    )
    if sent_record is not None:
        return NoticeOutcome.SKIPPED
    grace = await db.get(KycExecutionGrace, grace_id)
    user = await db.get(User, grace.user_id) if grace is not None else None
    if user is None or user.status != UserStatus.APPROVED or not user.email_verified:
        return NoticeOutcome.SKIPPED
    eligibility = await evaluate_kyc_execution(db, user, now=now)
    if eligibility.reason != ExecutionEligibilityReason.KYC_GRACE:
        return NoticeOutcome.SKIPPED

    payload = build_kyc_grace_notice_email_payload(
        user.email, user.first_name or "there", eligibility.grace_expires_at
    )
    if not await send_kyc_grace_notice_email(payload, grace.id):
        return NoticeOutcome.FAILED
    await record_audit(
        db,
        action=KYC_GRACE_NOTICE_SENT,
        resource_type=GRACE_NOTICE_RESOURCE_TYPE,
        resource_id=str(grace.id),
        changes={
            "user_id": str(user.id),
            "grace_expires_at": eligibility.grace_expires_at.isoformat(),
        },
    )
    return NoticeOutcome.SENT


async def send_kyc_grace_notices(
    db: AsyncSession, *, now: datetime | None = None
) -> dict[str, int]:
    """Send every unsent notice whose user still depends on grace."""
    report = {outcome.value: 0 for outcome in NoticeOutcome}
    for grace_id in await _unsent_grace_ids(db):
        outcome = await _send_one_notice(db, grace_id, now=now)
        # One transaction per notice: the record commits with its send.
        await db.commit()
        report[outcome.value] += 1
    return report

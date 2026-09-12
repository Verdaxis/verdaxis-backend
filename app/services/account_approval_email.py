"""Durable, transition-scoped delivery for account-approval emails."""

from __future__ import annotations

from copy import deepcopy
from datetime import UTC, datetime, timedelta
from typing import Literal
import uuid

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.user import User, UserRole, UserStatus
from app.services.email import send_account_approved_email


DEFAULT_APPROVAL_EMAIL_BATCH_SIZE = 20
MAX_APPROVAL_EMAIL_BATCH_SIZE = 100
# Leave headroom over the provider's 10-second phase timeouts, without a long crash delay.
APPROVAL_EMAIL_CLAIM_DURATION = timedelta(minutes=5)
APPROVAL_EMAIL_RETRY_DELAY = timedelta(hours=1)

DeliveryResult = Literal["sent", "deferred", "discarded", "skipped"]


def clear_pending_account_approval_email(user: User) -> None:
    """Clear all fields that belong to one pending approval notification."""
    user.pending_approval_email_transition_id = None
    user.pending_approval_email_payload = None
    user.pending_approval_email_retry_at = None


def _eligible_for_approval_email(user: User) -> bool:
    return (
        user.status == UserStatus.APPROVED
        and user.email_verified
        and user.role != UserRole.ADMIN
    )


async def deliver_account_approval_email(
    db: AsyncSession,
    *,
    user_id: uuid.UUID,
    transition_id: uuid.UUID,
) -> DeliveryResult:
    """Claim one due notification, send without a user lock, and finalize that claim."""
    now = datetime.now(UTC)
    user = (
        await db.execute(
            select(User)
            .where(
                User.id == user_id,
                User.pending_approval_email_transition_id == transition_id,
                User.pending_approval_email_retry_at <= now,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    if user is None:
        await db.rollback()
        return "skipped"

    if not _eligible_for_approval_email(user) or not isinstance(
        user.pending_approval_email_payload, dict
    ):
        clear_pending_account_approval_email(user)
        await db.commit()
        return "discarded"

    payload = deepcopy(user.pending_approval_email_payload)
    claim_until = datetime.now(UTC) + APPROVAL_EMAIL_CLAIM_DURATION
    user.pending_approval_email_retry_at = claim_until
    await db.commit()

    accepted = await send_account_approved_email(payload, transition_id)

    # A rejection, re-approval, or expired-lease retry can replace this claim during I/O.
    user = (
        await db.execute(
            select(User)
            .where(
                User.id == user_id,
                User.pending_approval_email_transition_id == transition_id,
                User.pending_approval_email_retry_at == claim_until,
            )
            .with_for_update()
            .execution_options(populate_existing=True)
        )
    ).scalar_one_or_none()
    if user is None:
        await db.rollback()
        return "skipped"

    if accepted:
        clear_pending_account_approval_email(user)
        await db.commit()
        return "sent"

    user.pending_approval_email_retry_at = datetime.now(UTC) + APPROVAL_EMAIL_RETRY_DELAY
    await db.commit()
    return "deferred"


async def retry_pending_account_approval_emails(
    db: AsyncSession,
    *,
    batch_size: int = DEFAULT_APPROVAL_EMAIL_BATCH_SIZE,
) -> dict[str, int]:
    """Retry a fair, bounded snapshot without holding the scan transaction."""
    if batch_size < 1 or batch_size > MAX_APPROVAL_EMAIL_BATCH_SIZE:
        raise ValueError(
            f"batch_size must be between 1 and {MAX_APPROVAL_EMAIL_BATCH_SIZE}"
        )

    now = datetime.now(UTC)
    rows = (
        await db.execute(
            select(
                User.id,
                User.pending_approval_email_transition_id,
            )
            .where(
                User.pending_approval_email_transition_id.is_not(None),
                User.pending_approval_email_retry_at <= now,
            )
            .order_by(User.pending_approval_email_retry_at, User.id)
            .limit(batch_size)
        )
    ).all()
    await db.rollback()

    completed = 0
    discarded = 0
    for row in rows:
        result = await deliver_account_approval_email(
            db,
            user_id=row.id,
            transition_id=row.pending_approval_email_transition_id,
        )
        completed += int(result == "sent")
        discarded += int(result == "discarded")

    return {
        "approval_emails_attempted": len(rows),
        "approval_emails_completed": completed,
        "approval_emails_discarded": discarded,
    }

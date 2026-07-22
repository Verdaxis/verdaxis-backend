"""Single batched fan-out for organization-wide notifications.

Stage 6c DRY consolidation: rfq, negotiations, trades, and the matching
engine each carried a hand-rolled copy of this fan-out; the matching
engine's batched variant (one user query per batch instead of one per
notification) is now the only implementation.
"""
from __future__ import annotations

import uuid

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.notification import Notification, NotificationType
from app.models.user import User

# (organization_id, type, title, message, data)
OrgNotification = tuple[uuid.UUID, NotificationType, str, str, dict | None]


async def notify_org_users_batched(
    db: AsyncSession, pending: list[OrgNotification]
) -> None:
    """Fan each entry out to every user of its organization.

    One user query covers the whole batch (the per-call version was an
    N+1: one query per notification).
    """
    if not pending:
        return
    org_ids = {entry[0] for entry in pending}
    result = await db.execute(select(User).where(User.organization_id.in_(org_ids)))
    users_by_org: dict[uuid.UUID, list[User]] = {}
    for user in result.scalars():
        users_by_org.setdefault(user.organization_id, []).append(user)

    for org_id, notif_type, title, message, data in pending:
        for user in users_by_org.get(org_id, []):
            db.add(
                Notification(
                    recipient_id=user.id,
                    type=notif_type,
                    title=title,
                    message=message,
                    data=data or {},
                )
            )


async def notify_org_users(
    db: AsyncSession,
    org_id: uuid.UUID,
    notif_type: NotificationType,
    title: str,
    message: str,
    data: dict | None = None,
) -> None:
    """Send one notification to every user in the given organization."""
    await notify_org_users_batched(db, [(org_id, notif_type, title, message, data)])

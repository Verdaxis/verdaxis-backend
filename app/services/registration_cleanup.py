"""Bounded cleanup for expired or consumed pending registrations."""

from datetime import UTC, datetime

from sqlalchemy import delete, or_
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.registration import PendingRegistration


async def cleanup_pending_registrations(
    db: AsyncSession, *, now: datetime | None = None
) -> int:
    cutoff = now or datetime.now(UTC)
    result = await db.execute(
        delete(PendingRegistration).where(
            or_(
                PendingRegistration.expires_at <= cutoff,
                PendingRegistration.used_at.is_not(None),
            )
        )
    )
    return result.rowcount or 0

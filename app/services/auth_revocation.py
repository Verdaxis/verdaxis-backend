"""Account-session revocation under the canonical locked User row."""

from datetime import UTC, datetime, timedelta

from sqlalchemy import update
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.refresh_session import RefreshSession
from app.models.user import User


def _as_utc(value: datetime) -> datetime:
    return value.replace(tzinfo=UTC) if value.tzinfo is None else value.astimezone(UTC)


async def invalidate_locked_user_authentication(
    db: AsyncSession,
    user: User,
    *,
    revoked_at: datetime | None = None,
) -> datetime:
    """Invalidate ordinary access and every refresh family for a locked user.

    The caller must hold this user's ``FOR UPDATE`` lock. This function does
    not change the password, emit a password audit event, or modify
    ``must_change_password``. The cutoff advances monotonically so an older
    clock value cannot revive a previously invalidated token.
    """
    cutoff = _as_utc(revoked_at or datetime.now(UTC))
    previous = user.authentication_revoked_at
    if previous is not None:
        previous = _as_utc(previous)
        if cutoff <= previous:
            cutoff = previous + timedelta(microseconds=1)
    user.authentication_revoked_at = cutoff
    await db.execute(
        update(RefreshSession)
        .where(RefreshSession.user_id == user.id)
        .values(revoked=True)
    )
    return cutoff

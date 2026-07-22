"""Bounded, independent maintenance for durable authentication state."""

from datetime import UTC, datetime, timedelta

from sqlalchemy import and_, delete, exists, inspect, or_, select, text, update
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import aliased

from app.models.refresh_session import RefreshSession
from app.models.registration import PendingRegistration
from app.models.user import User

DEFAULT_BATCH_SIZE = 1000
REVOKED_FAMILY_RETENTION = timedelta(hours=24)


async def _users_has_column(db: AsyncSession, column_name: str) -> bool:
    def inspect_columns(sync_session) -> bool:
        connection = sync_session.connection()
        return column_name in {
            column["name"] for column in inspect(connection).get_columns("users")
        }

    return await db.run_sync(inspect_columns)


async def run_auth_maintenance(
    db: AsyncSession,
    *,
    now: datetime | None = None,
    batch_size: int = DEFAULT_BATCH_SIZE,
) -> dict[str, int]:
    """Stage at most ``batch_size`` removals per auth-state category."""
    if batch_size < 1 or batch_size > 10_000:
        raise ValueError("batch_size must be between 1 and 10000")
    cutoff = now or datetime.now(UTC)

    active_successor = aliased(RefreshSession)
    refresh_ids = (
        select(RefreshSession.id)
        .where(
            or_(
                RefreshSession.expires_at <= cutoff,
                and_(
                    RefreshSession.revoked.is_(True),
                    RefreshSession.created_at <= cutoff - REVOKED_FAMILY_RETENTION,
                    ~exists(
                        select(active_successor.id).where(
                            active_successor.family_id == RefreshSession.family_id,
                            active_successor.revoked.is_(False),
                            active_successor.expires_at > cutoff,
                        )
                    ),
                ),
            )
        )
        .limit(batch_size)
    )
    refresh_result = await db.execute(
        delete(RefreshSession).where(RefreshSession.id.in_(refresh_ids))
    )

    pending_ids = (
        select(PendingRegistration.id)
        .where(
            or_(
                PendingRegistration.expires_at <= cutoff,
                PendingRegistration.used_at.is_not(None),
            )
        )
        .limit(batch_size)
    )
    pending_result = await db.execute(
        delete(PendingRegistration).where(PendingRegistration.id.in_(pending_ids))
    )

    reset_ids = (
        select(User.id)
        .where(
            User.password_reset_token_hash.is_not(None),
            or_(
                User.password_reset_expires.is_(None),
                User.password_reset_expires <= cutoff,
            ),
        )
        .limit(batch_size)
    )
    reset_result = await db.execute(
        update(User)
        .where(User.id.in_(reset_ids))
        .values(password_reset_token_hash=None, password_reset_expires=None)
    )

    email_ids = (
        select(User.id)
        .where(
            User.email_verification_token_hash.is_not(None),
            or_(
                User.email_verification_token_expires_at.is_(None),
                User.email_verification_token_expires_at <= cutoff,
            ),
        )
        .limit(batch_size)
    )
    email_result = await db.execute(
        update(User)
        .where(User.id.in_(email_ids))
        .values(
            email_verification_token_hash=None,
            email_verification_token_expires_at=None,
        )
    )

    legacy_plaintext_cleared = 0
    if await _users_has_column(db, "email_verification_token"):
        legacy_result = await db.execute(
            # This maintenance command is allowed to run during the explicit
            # identity-revision compatibility window. Never select or log the
            # plaintext values themselves.
            text(
                "UPDATE users SET email_verification_token = NULL WHERE id IN ("
                "SELECT id FROM users WHERE email_verification_token IS NOT NULL "
                "AND (email_verification_token_expires_at IS NULL "
                "OR email_verification_token_expires_at <= :cutoff) LIMIT :batch_size)"
            ),
            {"cutoff": cutoff, "batch_size": batch_size},
        )
        legacy_plaintext_cleared = legacy_result.rowcount or 0

    return {
        "refresh_sessions_deleted": refresh_result.rowcount or 0,
        "pending_registrations_deleted": pending_result.rowcount or 0,
        "password_reset_hashes_cleared": reset_result.rowcount or 0,
        "email_verification_hashes_cleared": email_result.rowcount or 0,
        "legacy_plaintext_email_tokens_cleared": legacy_plaintext_cleared,
    }

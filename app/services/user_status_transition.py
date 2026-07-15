"""Append-only account-status history writes (Product Analytics plan §2.4).

Every user-creation and status-mutation path routes through these helpers so
the transition fact is written in the same transaction as the ``User`` row:
either both commit or both roll back. A no-op (same status) writes nothing.
The initial interval is always reconstructed from the ``None →
initial_status`` transition — never from the mutable ``User.status``.
"""

from __future__ import annotations

from datetime import UTC, datetime

from sqlalchemy.ext.asyncio import AsyncSession

from app.models.product_analytics import (
    STATUS_TRANSITION_PROVENANCE_WORKFLOW,
    UserStatusTransition,
)
from app.models.user import User, UserStatus


def record_status_transition(
    db: AsyncSession,
    user: User,
    *,
    from_status: UserStatus | None,
    to_status: UserStatus,
    provenance: str = STATUS_TRANSITION_PROVENANCE_WORKFLOW,
    effective_at: datetime | None = None,
) -> UserStatusTransition | None:
    """Stage a transition row in the caller's transaction.

    Returns ``None`` for a no-op (``from_status == to_status``); the caller's
    commit/rollback covers both the status change and this row.
    """
    if from_status == to_status:
        return None
    row = UserStatusTransition(
        user_id=user.id,
        organization_id=user.organization_id,
        role=user.role,
        from_status=from_status,
        to_status=to_status,
        effective_at=effective_at or datetime.now(UTC),
        provenance=provenance,
    )
    db.add(row)
    return row


def record_initial_status(db: AsyncSession, user: User) -> UserStatusTransition:
    """Stage the ``None → initial_status`` row for a newly created user.

    Ordinary registration records ``None → PENDING``; privileged or system
    creation records whatever initial status it explicitly assigned. The user
    must be flushed (id assigned) before calling.
    """
    row = UserStatusTransition(
        user_id=user.id,
        organization_id=user.organization_id,
        role=user.role,
        from_status=None,
        to_status=user.status,
        effective_at=datetime.now(UTC),
        provenance=STATUS_TRANSITION_PROVENANCE_WORKFLOW,
    )
    db.add(row)
    return row

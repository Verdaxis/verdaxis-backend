"""Status-transition history correctness on PostgreSQL (plan §2.4).

Covers atomic initial and changed-state transitions, no-op behavior, rollback
of both status and transition together, the pending-interval-before-approval
reconstruction, rejection-then-reapproval, invariant historical reruns after
later transitions, and the migration's non-backdated snapshot rows.
"""

from __future__ import annotations

from datetime import UTC, datetime, timedelta
from uuid import uuid4

from sqlalchemy import func, select, text

from app.models.product_analytics import (
    STATUS_TRANSITION_PROVENANCE_MIGRATION,
    UserStatusTransition,
)
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.services.product_analytics import status_transition_facts_stmt
from app.services.user_status_transition import (
    record_initial_status,
    record_status_transition,
)


async def _new_user(session, *, status=UserStatus.PENDING, org_id=None) -> User:
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=status,
        organization_id=org_id,
        email_verified=True,
    )
    session.add(user)
    await session.flush()
    return user


async def test_initial_and_changed_transitions_commit_atomically(pg_session):
    _engine, session = pg_session
    user = await _new_user(session)
    record_initial_status(session, user)
    await session.commit()

    previous = user.status
    user.status = UserStatus.APPROVED
    record_status_transition(session, user, from_status=previous, to_status=UserStatus.APPROVED)
    await session.commit()

    rows = (
        (
            await session.execute(
                select(UserStatusTransition)
                .where(UserStatusTransition.user_id == user.id)
                .order_by(UserStatusTransition.effective_at)
            )
        )
        .scalars()
        .all()
    )
    assert [(row.from_status, row.to_status) for row in rows] == [
        (None, UserStatus.PENDING),
        (UserStatus.PENDING, UserStatus.APPROVED),
    ]
    assert all(row.provenance == "workflow" for row in rows)


async def test_noop_status_writes_create_no_transition(pg_session):
    _engine, session = pg_session
    user = await _new_user(session, status=UserStatus.APPROVED)
    record_initial_status(session, user)
    await session.commit()

    assert (
        record_status_transition(
            session, user, from_status=UserStatus.APPROVED, to_status=UserStatus.APPROVED
        )
        is None
    )
    await session.commit()
    count = await session.scalar(
        select(func.count(UserStatusTransition.id)).where(
            UserStatusTransition.user_id == user.id
        )
    )
    assert count == 1  # only the initial row


async def test_rollback_reverts_status_and_transition_together(pg_session):
    _engine, session = pg_session
    user = await _new_user(session)
    user_id = user.id
    record_initial_status(session, user)
    await session.commit()

    user.status = UserStatus.APPROVED
    record_status_transition(
        session, user, from_status=UserStatus.PENDING, to_status=UserStatus.APPROVED
    )
    await session.rollback()

    refreshed = (
        await session.execute(select(User).where(User.id == user_id))
    ).scalar_one()
    assert refreshed.status == UserStatus.PENDING
    count = await session.scalar(
        select(func.count(UserStatusTransition.id)).where(
            UserStatusTransition.user_id == user_id
        )
    )
    assert count == 1


async def test_rejection_then_reapproval_keeps_full_history_and_as_of_reruns_are_invariant(
    pg_session,
):
    _engine, session = pg_session
    org = Organization(id=uuid4(), name=f"Org {uuid4()}", type=OrgType.FUEL_BUYER)
    session.add(org)
    await session.flush()
    user = await _new_user(session, org_id=org.id)

    t0 = datetime(2026, 6, 1, 9, tzinfo=UTC)
    record_status_transition(
        session, user, from_status=None, to_status=UserStatus.PENDING, effective_at=t0
    )
    record_status_transition(
        session,
        user,
        from_status=UserStatus.PENDING,
        to_status=UserStatus.APPROVED,
        effective_at=t0 + timedelta(days=2),
    )
    record_status_transition(
        session,
        user,
        from_status=UserStatus.APPROVED,
        to_status=UserStatus.REJECTED,
        effective_at=t0 + timedelta(days=10),
    )
    await session.commit()

    async def qualified_as_of(as_of: datetime) -> int:
        row = (
            await session.execute(
                status_transition_facts_stmt(
                    as_of - timedelta(days=30), as_of, None, None
                )
            )
        ).one()
        return row.qualified_end

    # Pending interval before approval: not yet qualified.
    assert await qualified_as_of(t0 + timedelta(days=1)) == 0
    # Between approval and rejection: qualified.
    assert await qualified_as_of(t0 + timedelta(days=5)) == 1
    # After rejection: no longer qualified.
    assert await qualified_as_of(t0 + timedelta(days=11)) == 0

    # Reapproval appends; it never rewrites history.
    record_status_transition(
        session,
        user,
        from_status=UserStatus.REJECTED,
        to_status=UserStatus.APPROVED,
        effective_at=t0 + timedelta(days=20),
    )
    await session.commit()

    # Historical reruns after the later transition return identical values.
    assert await qualified_as_of(t0 + timedelta(days=1)) == 0
    assert await qualified_as_of(t0 + timedelta(days=5)) == 1
    assert await qualified_as_of(t0 + timedelta(days=11)) == 0
    assert await qualified_as_of(t0 + timedelta(days=21)) == 1


async def test_migration_snapshot_rows_exist_and_are_not_backdated(pg_session):
    """The deployment migration writes one migration_snapshot row per
    pre-existing Buyer/Supplier user, stamped at migration time."""
    engine, session = pg_session
    # Seed a member user, then re-run the snapshot INSERT the migration
    # performs (the migration itself ran against an empty database).
    org = Organization(id=uuid4(), name=f"Org {uuid4()}", type=OrgType.FUEL_BUYER)
    session.add(org)
    await session.flush()
    user = await _new_user(session, status=UserStatus.APPROVED, org_id=org.id)
    await session.commit()

    before = datetime.now(UTC)
    await session.execute(
        text(
            "INSERT INTO user_status_transitions "
            "(id, user_id, organization_id, role, from_status, to_status, "
            "effective_at, provenance) "
            "SELECT gen_random_uuid(), id, organization_id, role, NULL, status, "
            "now(), 'migration_snapshot' "
            "FROM users WHERE role IN ('BUYER', 'SUPPLIER')"
        )
    )
    await session.commit()

    rows = (
        (
            await session.execute(
                select(UserStatusTransition).where(
                    UserStatusTransition.provenance == STATUS_TRANSITION_PROVENANCE_MIGRATION
                )
            )
        )
        .scalars()
        .all()
    )
    assert len(rows) == 1
    snapshot = rows[0]
    assert snapshot.user_id == user.id
    assert snapshot.from_status is None
    assert snapshot.to_status == UserStatus.APPROVED
    assert snapshot.effective_at >= before  # never backdated


async def test_unique_and_index_constraints_exist_after_migration(pg_session):
    """Migration constraint/index assertions (plan Task 3 step 11)."""
    _engine, session = pg_session
    index_rows = (
        await session.execute(
            text(
                "SELECT indexname FROM pg_indexes WHERE tablename IN "
                "('user_login_days', 'user_status_transitions')"
            )
        )
    ).scalars().all()
    expected = {
        "uq_user_login_days_date_user",
        "ix_user_login_days_date_role",
        "ix_user_login_days_org_date",
        "ix_user_status_transitions_user_time",
        "ix_user_status_transitions_status_time",
    }
    assert expected <= set(index_rows), set(index_rows)

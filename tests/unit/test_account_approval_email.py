"""Durable account-approval email retry contracts."""

from datetime import UTC, datetime, timedelta
from pathlib import Path
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.user import Organization, User, UserRole, UserStatus
from app.services.account_approval_email import (
    deliver_account_approval_email,
    retry_pending_account_approval_emails,
)


ROOT = Path(__file__).resolve().parents[2]


def test_approval_email_migration_and_checkpoint_are_linear():
    migration = ROOT / "alembic/versions/ae_20260811_account_approval_email.py"
    source = migration.read_text()
    checkpoints = (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()

    assert 'revision = "ae_20260811_approval_email"' in source
    assert 'down_revision = "fb_20260804_feedback_entries"' in source
    assert '"pending_approval_email_transition_id"' in source
    assert '"pending_approval_email_payload"' in source
    assert '"pending_approval_email_retry_at"' in source
    assert "ix_users_pending_approval_email_retry" in source
    assert "fb_20260804_feedback_entries\tae_20260811_approval_email" in checkpoints
    assert "ae_20260811_approval_email\tai_20260831_invite_real_orgs" in checkpoints


def test_staging_environment_example_uses_staging_frontend():
    example = (ROOT / ".env.example").read_text()
    assert "ENVIRONMENT=staging" in example
    assert "FRONTEND_URL=https://staging.verdaxis.exchange" in example


@pytest.fixture
async def approval_email_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as connection:
        await connection.run_sync(
            Base.metadata.create_all,
            tables=[Organization.__table__, User.__table__],
        )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


async def _pending_user(
    db: AsyncSession,
    *,
    role: UserRole | None = UserRole.BUYER,
    status: UserStatus = UserStatus.APPROVED,
    email_verified: bool = True,
) -> User:
    user_id = uuid4()
    email = f"{user_id}@example.test"
    user = User(
        id=user_id,
        email=email,
        password_hash="hash",
        first_name="Approval-time name",
        role=role,
        status=status,
        email_verified=email_verified,
        pending_approval_email_transition_id=uuid4(),
        pending_approval_email_payload={
            "from": "Verdaxis <approval@example.test>",
            "to": [email],
            "subject": "Your Verdaxis account has been approved",
            "html": "<p>Hi Approval-time name</p>",
        },
        pending_approval_email_retry_at=datetime.now(UTC) - timedelta(minutes=1),
    )
    db.add(user)
    await db.commit()
    return user


@pytest.mark.asyncio
async def test_retry_replays_frozen_payload_and_clears_pending_state(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    user = await _pending_user(approval_email_db)
    transition_id = user.pending_approval_email_transition_id
    frozen_payload = dict(user.pending_approval_email_payload)
    user.first_name = "Changed after approval"
    await approval_email_db.commit()

    async def send_without_transaction(*_args):
        assert not approval_email_db.in_transaction()
        return True

    send = AsyncMock(side_effect=send_without_transaction)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    report = await retry_pending_account_approval_emails(approval_email_db, batch_size=20)

    assert report == {
        "approval_emails_attempted": 1,
        "approval_emails_completed": 1,
        "approval_emails_discarded": 0,
    }
    send.assert_awaited_once_with(frozen_payload, transition_id)
    await approval_email_db.refresh(user)
    assert user.pending_approval_email_transition_id is None
    assert user.pending_approval_email_payload is None
    assert user.pending_approval_email_retry_at is None


@pytest.mark.asyncio
async def test_interrupted_delivery_keeps_a_claim_that_can_be_retried(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    user = await _pending_user(approval_email_db)
    user_id = user.id
    transition_id = user.pending_approval_email_transition_id
    frozen_payload = dict(user.pending_approval_email_payload)
    send = AsyncMock(side_effect=RuntimeError("interrupted worker"))
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email", send
    )

    with pytest.raises(RuntimeError, match="interrupted worker"):
        await deliver_account_approval_email(
            approval_email_db, user_id=user_id, transition_id=transition_id
        )

    assert not approval_email_db.in_transaction()
    await approval_email_db.refresh(user)
    retry_at = user.pending_approval_email_retry_at.replace(tzinfo=UTC)
    assert retry_at > datetime.now(UTC)
    assert user.pending_approval_email_transition_id == transition_id
    assert user.pending_approval_email_payload == frozen_payload

    # Simulate the persisted lease expiring before the next maintenance run.
    user.pending_approval_email_retry_at = datetime.now(UTC) - timedelta(seconds=1)
    await approval_email_db.commit()
    send.side_effect = None
    send.return_value = True
    result = await deliver_account_approval_email(
        approval_email_db, user_id=user_id, transition_id=transition_id
    )

    assert result == "sent"
    assert send.await_count == 2
    send.assert_awaited_with(frozen_payload, transition_id)
    await approval_email_db.refresh(user)
    assert user.pending_approval_email_transition_id is None


@pytest.mark.asyncio
@pytest.mark.parametrize("accepted", [True, False])
async def test_stale_completion_preserves_a_newer_claim_for_the_same_transition(
    approval_email_db: AsyncSession,
    monkeypatch,
    accepted: bool,
):
    user = await _pending_user(approval_email_db)
    user_id = user.id
    transition_id = user.pending_approval_email_transition_id
    frozen_payload = dict(user.pending_approval_email_payload)
    newer_retry_at = datetime.now(UTC) + timedelta(minutes=10)

    async def reclaim_during_send(*_args):
        async with AsyncSession(
            approval_email_db.bind, expire_on_commit=False
        ) as newer_session:
            newer_user = await newer_session.get(User, user_id)
            newer_user.pending_approval_email_retry_at = newer_retry_at
            await newer_session.commit()
        return accepted

    send = AsyncMock(side_effect=reclaim_during_send)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email", send
    )
    result = await deliver_account_approval_email(
        approval_email_db, user_id=user_id, transition_id=transition_id
    )

    assert result == "skipped"
    send.assert_awaited_once_with(frozen_payload, transition_id)
    await approval_email_db.refresh(user)
    assert user.pending_approval_email_transition_id == transition_id
    assert user.pending_approval_email_payload == frozen_payload
    assert user.pending_approval_email_retry_at.replace(tzinfo=UTC) == newer_retry_at


@pytest.mark.asyncio
async def test_provider_failure_defers_row_so_it_cannot_starve_newer_work(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    users = [await _pending_user(approval_email_db) for _ in range(21)]
    send = AsyncMock(return_value=False)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    retry_not_before = datetime.now(UTC) + timedelta(hours=1)
    first = await retry_pending_account_approval_emails(approval_email_db, batch_size=20)
    second = await retry_pending_account_approval_emails(approval_email_db, batch_size=20)

    assert first["approval_emails_attempted"] == 20
    assert second["approval_emails_attempted"] == 1
    assert send.await_count == 21
    for user in users:
        await approval_email_db.refresh(user)
        assert user.pending_approval_email_transition_id is not None
        retry_at = user.pending_approval_email_retry_at
        if retry_at.tzinfo is None:  # SQLite drops timezone metadata in unit tests.
            retry_at = retry_at.replace(tzinfo=UTC)
        assert retry_at >= retry_not_before


@pytest.mark.asyncio
async def test_rejected_account_is_discarded_before_provider_call(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    user = await _pending_user(approval_email_db, status=UserStatus.REJECTED)
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    report = await retry_pending_account_approval_emails(approval_email_db, batch_size=20)

    assert report == {
        "approval_emails_attempted": 1,
        "approval_emails_completed": 0,
        "approval_emails_discarded": 1,
    }
    send.assert_not_awaited()
    await approval_email_db.refresh(user)
    assert user.pending_approval_email_transition_id is None


@pytest.mark.asyncio
async def test_delivery_refreshes_stale_identity_state_before_sending(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    user = await _pending_user(approval_email_db)
    transition_id = user.pending_approval_email_transition_id
    async with AsyncSession(
        approval_email_db.bind,
        expire_on_commit=False,
    ) as rejecting_session:
        rejected = await rejecting_session.get(User, user.id)
        rejected.status = UserStatus.REJECTED
        await rejecting_session.commit()
    assert user.status == UserStatus.APPROVED  # Deliberately stale identity map.
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    result = await deliver_account_approval_email(
        approval_email_db,
        user_id=user.id,
        transition_id=transition_id,
    )

    assert result == "discarded"
    send.assert_not_awaited()
    await approval_email_db.refresh(user)
    assert user.status == UserStatus.REJECTED
    assert user.pending_approval_email_transition_id is None


@pytest.mark.asyncio
async def test_legacy_null_role_is_still_eligible_for_delivery(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    user = await _pending_user(approval_email_db, role=None)
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    report = await retry_pending_account_approval_emails(approval_email_db, batch_size=20)

    assert report["approval_emails_completed"] == 1
    await approval_email_db.refresh(user)
    assert user.pending_approval_email_transition_id is None


@pytest.mark.asyncio
async def test_stale_transition_cannot_send_or_clear_newer_marker(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    user = await _pending_user(approval_email_db)
    newer_transition_id = user.pending_approval_email_transition_id
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    result = await deliver_account_approval_email(
        approval_email_db,
        user_id=user.id,
        transition_id=uuid4(),
    )

    assert result == "skipped"
    send.assert_not_awaited()
    await approval_email_db.refresh(user)
    assert user.pending_approval_email_transition_id == newer_transition_id


@pytest.mark.asyncio
async def test_retry_discards_other_ineligible_pending_markers(
    approval_email_db: AsyncSession,
    monkeypatch,
):
    await _pending_user(approval_email_db, role=UserRole.ADMIN)
    await _pending_user(approval_email_db, email_verified=False)
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        send,
    )

    report = await retry_pending_account_approval_emails(approval_email_db, batch_size=20)

    assert report["approval_emails_attempted"] == 2
    assert report["approval_emails_discarded"] == 2
    send.assert_not_awaited()


@pytest.mark.asyncio
async def test_retry_rejects_unbounded_batches(approval_email_db: AsyncSession):
    with pytest.raises(ValueError):
        await retry_pending_account_approval_emails(approval_email_db, batch_size=0)
    with pytest.raises(ValueError):
        await retry_pending_account_approval_emails(approval_email_db, batch_size=101)

"""PostgreSQL serialization checks for account-approval notification delivery."""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.models.user import User, UserRole, UserStatus
from app.services.account_approval_email import (
    clear_pending_account_approval_email,
    deliver_account_approval_email,
)


@pytest.mark.asyncio
async def test_rejection_waits_for_row_locked_approval_email_delivery(
    pg_session,
    monkeypatch,
):
    engine, seed_session = pg_session
    transition_id = uuid4()
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        pending_approval_email_transition_id=transition_id,
        pending_approval_email_payload={
            "from": "Verdaxis <approval@example.test>",
            "to": ["recipient@example.test"],
            "subject": "Your Verdaxis account has been approved",
            "html": "<p>Approved</p>",
        },
        pending_approval_email_retry_at=datetime.now(UTC) - timedelta(minutes=1),
    )
    user_id = user.id
    seed_session.add(user)
    await seed_session.commit()

    send_started = asyncio.Event()
    release_send = asyncio.Event()

    async def blocked_send(*_args):
        send_started.set()
        await release_send.wait()
        return True

    monkeypatch.setattr(
        "app.services.account_approval_email.send_account_approved_email",
        AsyncMock(side_effect=blocked_send),
    )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)

    async def deliver():
        async with factory() as session:
            return await deliver_account_approval_email(
                session,
                user_id=user_id,
                transition_id=transition_id,
            )

    async def reject():
        async with factory() as session:
            target = await session.scalar(
                select(User).where(User.id == user_id).with_for_update()
            )
            target.status = UserStatus.REJECTED
            clear_pending_account_approval_email(target)
            await session.commit()

    delivery_task = asyncio.create_task(deliver())
    await asyncio.wait_for(send_started.wait(), timeout=2)
    rejection_task = asyncio.create_task(reject())
    await asyncio.sleep(0.1)
    assert not rejection_task.done()

    release_send.set()
    assert await delivery_task == "sent"
    await asyncio.wait_for(rejection_task, timeout=2)

    seed_session.expire_all()
    final = await seed_session.get(User, user_id)
    assert final.status == UserStatus.REJECTED
    assert final.pending_approval_email_transition_id is None

"""Concurrent same-day login upsert correctness on PostgreSQL (plan §2.4).

Simultaneous successful logins for one user must resolve to exactly one
daily row carrying the combined login count, with no authentication
failure — the ON CONFLICT upsert, not application-level read-modify-write,
is the concurrency mechanism.
"""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime
from uuid import uuid4

from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.models.product_analytics import UserLoginDay
from app.models.user import User, UserRole, UserStatus
from app.services.product_analytics import record_login_day

_CONCURRENT_LOGINS = 8


async def test_simultaneous_logins_collapse_to_one_row_with_exact_count(pg_session):
    engine, session = pg_session
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    session.add(user)
    await session.commit()

    base = datetime(2026, 7, 15, 9, tzinfo=UTC)

    async def one_login(offset_minutes: int) -> None:
        # Each login uses its own session/connection, exactly like separate
        # request handlers hitting the database at once.
        worker = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
        async with worker() as login_session:
            db_user = await login_session.get(User, user.id)
            instant = base.replace(minute=offset_minutes)
            db_user.last_login = instant
            await record_login_day(login_session, db_user, at=instant)
            await login_session.commit()

    await asyncio.gather(*(one_login(minute) for minute in range(_CONCURRENT_LOGINS)))

    rows = (
        (await session.execute(select(UserLoginDay).where(UserLoginDay.user_id == user.id)))
        .scalars()
        .all()
    )
    assert len(rows) == 1
    row = rows[0]
    assert row.login_count == _CONCURRENT_LOGINS
    assert row.activity_date == base.date()
    assert row.first_login_at == base.replace(minute=0)
    assert row.last_login_at == base.replace(minute=_CONCURRENT_LOGINS - 1)

    total = await session.scalar(select(func.count(UserLoginDay.id)))
    assert total == 1

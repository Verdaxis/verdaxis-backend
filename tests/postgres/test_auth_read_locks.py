"""PostgreSQL proofs for authenticated read and account-state lock ordering."""

import asyncio
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request
from sqlalchemy import update
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.core.security import create_access_token
from app.models.user import User, UserRole, UserStatus
from app.routers.auth_simple import (
    _resolve_authenticated_user,
    validate_authenticated_user_state,
)


def _read_request() -> Request:
    return Request(
        {
            "type": "http",
            "method": "GET",
            "path": "/api/auth/me",
            "headers": [],
            "query_string": b"",
            "scheme": "https",
            "server": ("test", 443),
            "client": ("test", 1234),
        }
    )


@pytest.mark.asyncio
async def test_authenticated_reads_overlap_and_status_update_waits(pg_session):
    engine, seed_session = pg_session
    user = User(
        id=uuid4(),
        email=f"auth-lock-{uuid4()}@example.test",
        password_hash="unused",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    seed_session.add(user)
    await seed_session.commit()

    token = create_access_token(str(user.id))
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    release_reads = asyncio.Event()
    first_acquired = asyncio.Event()
    second_acquired = asyncio.Event()
    writer_started = asyncio.Event()
    writer_acquired = asyncio.Event()

    async def read(acquired: asyncio.Event) -> None:
        async with factory() as session:
            loaded, payload = await _resolve_authenticated_user(
                _read_request(), token, session
            )
            validate_authenticated_user_state(
                loaded, payload, request_path="/api/auth/me"
            )
            acquired.set()
            await release_reads.wait()
            await session.rollback()

    async def reject_account() -> None:
        await asyncio.gather(first_acquired.wait(), second_acquired.wait())
        async with factory() as session:
            writer_started.set()
            await session.execute(
                update(User)
                .where(User.id == user.id)
                .values(status=UserStatus.REJECTED)
            )
            writer_acquired.set()
            await session.commit()

    first = asyncio.create_task(read(first_acquired))
    await asyncio.wait_for(first_acquired.wait(), timeout=1)
    second = asyncio.create_task(read(second_acquired))
    await asyncio.wait_for(second_acquired.wait(), timeout=1)
    writer = asyncio.create_task(reject_account())
    await asyncio.wait_for(writer_started.wait(), timeout=1)

    try:
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(writer_acquired.wait(), timeout=0.2)
    finally:
        release_reads.set()

    await asyncio.gather(first, second, writer)
    assert writer_acquired.is_set()

    async with factory() as session:
        rejected, payload = await _resolve_authenticated_user(
            _read_request(), token, session
        )
        with pytest.raises(HTTPException) as exc_info:
            validate_authenticated_user_state(
                rejected, payload, request_path="/api/auth/me"
            )
        assert exc_info.value.status_code == 403

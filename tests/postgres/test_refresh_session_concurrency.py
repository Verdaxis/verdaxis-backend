"""PostgreSQL concurrency coverage for durable refresh rotation."""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.core.security import (
    create_access_token,
    create_refresh_token,
    decode_token,
    hash_token_identifier,
)
from app.models.refresh_session import RefreshSession
from app.models.user import User, UserRole, UserStatus
from app.routers.auth_simple import (
    _acquire_device_session_lock,
    _device_session_hash,
    _resolve_authenticated_user,
    _revoke_refresh_family,
    _rotate_refresh_session,
)
from app.services.auth_revocation import invalidate_locked_user_authentication

DEVICE_HASH = "a" * 64


@pytest.mark.asyncio
async def test_concurrent_refresh_duplicate_preserves_successor_and_late_replay_revokes_family(pg_session):
    engine, session = pg_session
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    family_id = uuid4()
    original = create_refresh_token(subject=str(user.id), family_id=str(family_id))
    original_payload = decode_token(original)
    session.add(user)
    await session.flush()
    session.add(
        RefreshSession(
            user_id=user.id,
            family_id=family_id,
            jti_hash=hash_token_identifier(original_payload["jti"]),
            device_id_hash=DEVICE_HASH,
            expires_at=datetime.fromtimestamp(original_payload["exp"], tz=UTC),
        )
    )
    await session.commit()

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)

    async def rotate_once() -> str:
        async with factory() as worker:
            successor = create_refresh_token(subject=str(user.id), family_id=str(family_id))
            try:
                await _rotate_refresh_session(
                    worker,
                    user.id,
                    original_payload,
                    successor,
                    device_id_hash=DEVICE_HASH,
                )
                await worker.commit()
                return "rotated"
            except HTTPException as exc:
                await worker.rollback()
                return exc.detail["code"] if isinstance(exc.detail, dict) else str(exc.detail)

    outcomes = await asyncio.gather(rotate_once(), rotate_once())
    assert outcomes.count("rotated") == 1
    assert outcomes.count("REFRESH_ROTATION_IN_PROGRESS") == 1

    old = await session.scalar(
        select(RefreshSession).where(RefreshSession.jti_hash == hash_token_identifier(original_payload["jti"]))
    )
    assert old is not None
    assert old.replaced_by_jti_hash is not None
    assert old.revoked is True

    old.rotation_grace_until = datetime.now(UTC) - timedelta(seconds=1)
    await session.commit()
    async with factory() as replay_session:
        with pytest.raises(HTTPException) as exc_info:
            await _rotate_refresh_session(
                replay_session,
                user.id,
                original_payload,
                create_refresh_token(subject=str(user.id), family_id=str(family_id)),
                device_id_hash=DEVICE_HASH,
            )
        assert exc_info.value.status_code == 401
        assert exc_info.value.detail["code"] == "REFRESH_TOKEN_REPLAYED"

    session.expire_all()
    family = (await session.execute(select(RefreshSession).where(RefreshSession.family_id == family_id))).scalars().all()
    assert family
    assert all(row.revoked for row in family)


@pytest.mark.asyncio
async def test_logout_with_predecessor_serializes_with_successor_rotation(pg_session):
    """An older family token must still revoke a concurrently-created successor."""
    engine, session = pg_session
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    family_id = uuid4()
    original = create_refresh_token(subject=str(user.id), family_id=str(family_id))
    original_payload = decode_token(original)
    session.add(user)
    await session.flush()
    session.add(
        RefreshSession(
            user_id=user.id,
            family_id=family_id,
            jti_hash=hash_token_identifier(original_payload["jti"]),
            device_id_hash=DEVICE_HASH,
            expires_at=datetime.fromtimestamp(original_payload["exp"], tz=UTC),
        )
    )
    await session.commit()

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    successor = create_refresh_token(subject=str(user.id), family_id=str(family_id))
    async with factory() as setup_worker:
        await _rotate_refresh_session(
            setup_worker,
            user.id,
            original_payload,
            successor,
            device_id_hash=DEVICE_HASH,
        )
        await setup_worker.commit()

    successor_payload = decode_token(successor)
    rotation_staged = asyncio.Event()
    allow_rotation_commit = asyncio.Event()

    async def rotate_successor() -> None:
        async with factory() as worker:
            next_token = create_refresh_token(subject=str(user.id), family_id=str(family_id))
            await _rotate_refresh_session(
                worker,
                user.id,
                successor_payload,
                next_token,
                device_id_hash=DEVICE_HASH,
            )
            rotation_staged.set()
            await allow_rotation_commit.wait()
            await worker.commit()

    async def logout_with_original() -> None:
        await rotation_staged.wait()
        async with factory() as worker:
            await _revoke_refresh_family(worker, original_payload)
            await worker.commit()

    rotation_task = asyncio.create_task(rotate_successor())
    await rotation_staged.wait()
    logout_task = asyncio.create_task(logout_with_original())
    await asyncio.sleep(0.1)
    assert not logout_task.done(), "logout must wait for the family's user-row lock"
    allow_rotation_commit.set()
    await asyncio.gather(rotation_task, logout_task)

    session.expire_all()
    family = (
        await session.execute(
            select(RefreshSession).where(RefreshSession.family_id == family_id)
        )
    ).scalars().all()
    assert len(family) == 3
    assert all(row.revoked for row in family)


@pytest.mark.asyncio
async def test_rejection_waits_for_refresh_and_revokes_its_successor(pg_session):
    engine, session = pg_session
    user = User(
        id=uuid4(),
        email=f"reject-race-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    user_id = user.id
    family_id = uuid4()
    original = create_refresh_token(subject=str(user_id), family_id=str(family_id))
    original_payload = decode_token(original)
    session.add(user)
    await session.flush()
    session.add(
        RefreshSession(
            user_id=user.id,
            family_id=family_id,
            jti_hash=hash_token_identifier(original_payload["jti"]),
            device_id_hash=DEVICE_HASH,
            expires_at=datetime.fromtimestamp(original_payload["exp"], tz=UTC),
        )
    )
    await session.commit()

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    rotation_staged = asyncio.Event()
    allow_rotation_commit = asyncio.Event()
    rejection_acquired_user = asyncio.Event()

    async def rotate() -> None:
        async with factory() as worker:
            successor = create_refresh_token(
                subject=str(user.id), family_id=str(family_id)
            )
            await _rotate_refresh_session(
                worker,
                user.id,
                original_payload,
                successor,
                device_id_hash=DEVICE_HASH,
            )
            rotation_staged.set()
            await allow_rotation_commit.wait()
            await worker.commit()

    async def reject() -> None:
        await rotation_staged.wait()
        async with factory() as worker:
            locked_user = (
                await worker.execute(
                    select(User).where(User.id == user.id).with_for_update()
                )
            ).scalar_one()
            rejection_acquired_user.set()
            locked_user.status = UserStatus.REJECTED
            await invalidate_locked_user_authentication(worker, locked_user)
            await worker.commit()

    rotation_task = asyncio.create_task(rotate())
    await rotation_staged.wait()
    rejection_task = asyncio.create_task(reject())
    try:
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(rejection_acquired_user.wait(), timeout=0.2)
    finally:
        allow_rotation_commit.set()
    await asyncio.gather(rotation_task, rejection_task)

    session.expire_all()
    rejected_user = await session.get(User, user_id)
    family = (
        await session.execute(
            select(RefreshSession).where(RefreshSession.family_id == family_id)
        )
    ).scalars().all()
    assert rejected_user.status == UserStatus.REJECTED
    assert rejected_user.authentication_revoked_at is not None
    assert len(family) == 2
    assert all(row.revoked for row in family)

@pytest.mark.asyncio
async def test_password_change_authentication_locks_device_before_user(pg_session):
    engine, session = pg_session
    user = User(
        id=uuid4(),
        email=f"password-lock-order-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    session.add(user)
    await session.commit()

    raw_device_id = "a" * 43
    device_id_hash = _device_session_hash(raw_device_id)
    token = create_access_token(str(user.id))
    request = Request(
        {
            "type": "http",
            "method": "PUT",
            "path": "/api/auth/me/password",
            "query_string": b"",
            "headers": [
                (b"cookie", f"device_session={raw_device_id}".encode()),
            ],
            "scheme": "https",
            "server": ("test", 443),
            "client": ("test", 1234),
        }
    )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    holder = factory()
    await _acquire_device_session_lock(holder, device_id_hash)

    async def authenticate_password_change():
        async with factory() as worker:
            loaded, _payload = await _resolve_authenticated_user(
                request, token, worker
            )
            return loaded.id

    authentication = asyncio.create_task(authenticate_password_change())
    try:
        await asyncio.sleep(0.1)
        assert not authentication.done()
        locked_user = (
            await holder.execute(
                select(User)
                .where(User.id == user.id)
                .with_for_update(nowait=True)
            )
        ).scalar_one()
        assert locked_user.id == user.id
        await holder.commit()
        assert await asyncio.wait_for(authentication, timeout=2) == user.id
    finally:
        if holder.in_transaction():
            await holder.rollback()
        await holder.close()
        if not authentication.done():
            authentication.cancel()
            with pytest.raises(asyncio.CancelledError):
                await authentication

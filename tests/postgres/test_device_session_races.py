"""Real PostgreSQL races for browser-device refresh-family replacement."""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime
from uuid import UUID, uuid4

import pytest
from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.core.security import create_refresh_token, decode_token, get_password_hash, hash_token_identifier
from app.models.refresh_session import RefreshSession
from app.models.user import User, UserRole, UserStatus
from app.routers import auth_simple


async def _seed_device_family(session: AsyncSession):
    password = "correct horse battery staple"
    user_a = User(
        id=uuid4(),
        email=f"account-a-{uuid4()}@example.test",
        password_hash=get_password_hash(password),
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    user_b = User(
        id=uuid4(),
        email=f"account-b-{uuid4()}@example.test",
        password_hash=get_password_hash(password),
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
    )
    raw_device_id = auth_simple._new_device_session_id()
    device_hash = auth_simple._device_session_hash(raw_device_id)
    original = create_refresh_token(str(user_a.id))
    payload = decode_token(original)
    session.add_all((user_a, user_b))
    await session.flush()
    session.add(
        RefreshSession(
            user_id=user_a.id,
            family_id=UUID(payload["family_id"]),
            jti_hash=hash_token_identifier(payload["jti"]),
            device_id_hash=device_hash,
            expires_at=datetime.fromtimestamp(payload["exp"], tz=UTC),
        )
    )
    await session.commit()
    return user_a, user_b, original, payload, device_hash


async def _rotate(
    db: AsyncSession,
    *,
    user_id,
    token: str,
    device_hash: str,
) -> str:
    payload = decode_token(token)
    successor = create_refresh_token(str(user_id), family_id=payload["family_id"])
    await auth_simple._rotate_refresh_session(
        db,
        user_id,
        payload,
        successor,
        device_id_hash=device_hash,
    )
    return successor


async def _login_replacement(
    db: AsyncSession,
    *,
    user_id,
    device_hash: str,
    presented_payload: dict,
) -> str:
    await auth_simple._acquire_device_session_lock(db, device_hash)
    await auth_simple._revoke_device_refresh_sessions(
        db,
        device_hash,
        presented_payload=presented_payload,
    )
    token = create_refresh_token(str(user_id))
    await auth_simple._store_refresh_session(
        db,
        user_id,
        token,
        device_id_hash=device_hash,
    )
    return token


async def _logout_replacement(db: AsyncSession, *, device_hash: str, presented_payload: dict) -> None:
    await auth_simple._revoke_device_refresh_sessions(
        db,
        device_hash,
        presented_payload=presented_payload,
    )


async def _rotation_code(factory, *, user_id, token: str, device_hash: str) -> str:
    async with factory() as worker:
        try:
            await _rotate(worker, user_id=user_id, token=token, device_hash=device_hash)
            await worker.commit()
            return "ROTATED"
        except HTTPException as exc:
            await worker.rollback()
            return exc.detail["code"]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("first_operation", "second_operation"),
    (
        ("refresh", "login"),
        ("login", "refresh"),
        ("refresh", "logout"),
        ("logout", "refresh"),
    ),
)
async def test_device_lock_makes_delayed_account_a_cookie_unusable_in_both_lock_orders(
    pg_session,
    first_operation,
    second_operation,
):
    engine, session = pg_session
    user_a, user_b, original, original_payload, device_hash = await _seed_device_family(session)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    first_staged = asyncio.Event()
    allow_first_commit = asyncio.Event()

    async def run_operation(db: AsyncSession, operation: str):
        if operation == "refresh":
            return await _rotate(
                db,
                user_id=user_a.id,
                token=original,
                device_hash=device_hash,
            )
        if operation == "login":
            return await _login_replacement(
                db,
                user_id=user_b.id,
                device_hash=device_hash,
                presented_payload=original_payload,
            )
        await _logout_replacement(
            db,
            device_hash=device_hash,
            presented_payload=original_payload,
        )
        return None

    async def first():
        async with factory() as worker:
            # Pre-acquisition creates a deterministic pause after this operation
            # owns the exact same transaction lock used by the endpoint helper.
            await auth_simple._acquire_device_session_lock(worker, device_hash)
            result = await run_operation(worker, first_operation)
            first_staged.set()
            await allow_first_commit.wait()
            await worker.commit()
            return result

    async def second():
        await first_staged.wait()
        async with factory() as worker:
            try:
                result = await run_operation(worker, second_operation)
                await worker.commit()
                return result
            except HTTPException as exc:
                await worker.rollback()
                return exc.detail["code"]

    first_task = asyncio.create_task(first())
    await first_staged.wait()
    second_task = asyncio.create_task(second())
    await asyncio.sleep(0.1)
    assert not second_task.done(), "the second browser operation must wait on the device advisory lock"
    allow_first_commit.set()
    first_result, second_result = await asyncio.gather(first_task, second_task)

    delayed_a_token = first_result if first_operation == "refresh" else original
    assert await _rotation_code(
        factory,
        user_id=user_a.id,
        token=delayed_a_token,
        device_hash=device_hash,
    ) in {"REFRESH_SESSION_REVOKED", "REFRESH_TOKEN_REPLAYED"}

    if "login" in (first_operation, second_operation):
        login_token = first_result if first_operation == "login" else second_result
        assert await _rotation_code(
            factory,
            user_id=user_b.id,
            token=login_token,
            device_hash=device_hash,
        ) == "ROTATED"
    else:
        assert second_result != "ROTATED"


@pytest.mark.asyncio
async def test_legacy_null_device_family_never_rotates_into_a_new_device(pg_session):
    engine, session = pg_session
    user_a, _user_b, original, payload, device_hash = await _seed_device_family(session)
    row = await session.scalar(
        select(RefreshSession).where(RefreshSession.jti_hash == hash_token_identifier(payload["jti"]))
    )
    row.device_id_hash = None
    row.revoked = False
    await session.commit()
    row_id = row.id
    family_id = row.family_id

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as worker:
        with pytest.raises(HTTPException) as exc_info:
            await _rotate(
                worker,
                user_id=user_a.id,
                token=original,
                device_hash=device_hash,
            )
        assert exc_info.value.detail["code"] == "REFRESH_SESSION_REVOKED"
        await worker.rollback()

    session.expire_all()
    legacy_row = await session.scalar(
        select(RefreshSession).where(RefreshSession.id == row_id)
    )
    assert legacy_row.device_id_hash is None
    assert await session.scalar(
        select(RefreshSession.id).where(RefreshSession.family_id == family_id)
    ) == row_id

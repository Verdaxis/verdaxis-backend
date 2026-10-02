"""PostgreSQL proofs for rejection-time account-session revocation."""

from typing import Annotated
from uuid import uuid4

import httpx
import pytest
from fastapi import Depends, FastAPI
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.core.security import decode_token, get_password_hash, hash_token_identifier
from app.database import get_db
from app.models.refresh_session import RefreshSession
from app.models.user import User, UserRole, UserStatus
from app.rate_limit import limiter
from app.routers.auth_simple import get_authenticated_user, router as auth_router
from app.services.auth_revocation import invalidate_locked_user_authentication


@pytest.mark.asyncio
async def test_reject_reapprove_never_revives_old_tokens_and_fresh_login_works(
    pg_session,
    monkeypatch,
):
    engine, seed_session = pg_session
    password = "correct horse battery staple 9"
    user = User(
        id=uuid4(),
        email=f"rejection-{uuid4()}@example.test",
        password_hash=get_password_hash(password),
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
    )
    seed_session.add(user)
    await seed_session.commit()
    user_id = user.id

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    app = FastAPI()
    app.state.limiter = limiter
    app.include_router(auth_router, prefix="/api")

    @app.get("/owner-cleanup-proof")
    async def owner_cleanup_proof(
        current_user: Annotated[User, Depends(get_authenticated_user)],
    ):
        return {"user_id": str(current_user.id)}

    async def override_db():
        async with factory() as session:
            yield session

    app.dependency_overrides[get_db] = override_db
    monkeypatch.setattr(limiter, "enabled", False)
    transport = httpx.ASGITransport(app=app, raise_app_exceptions=False)

    async with httpx.AsyncClient(
        transport=transport,
        base_url="https://test",
        headers={"Origin": "https://test"},
    ) as client:
        login_response = await client.post(
            "/api/auth/login",
            data={"username": user.email, "password": password},
        )
        assert login_response.status_code == 200, login_response.text
        old_access_token = login_response.json()["access_token"]
        old_refresh_token = client.cookies.get("refresh_token")
        old_device_id = client.cookies.get("device_session")
        assert old_refresh_token and old_device_id
        old_refresh_hash = hash_token_identifier(
            decode_token(old_refresh_token)["jti"]
        )

        async with factory() as session:
            locked_user = (
                await session.execute(
                    select(User).where(User.id == user_id).with_for_update()
                )
            ).scalar_one()
            locked_user.status = UserStatus.REJECTED
            cutoff = await invalidate_locked_user_authentication(session, locked_user)
            await session.commit()

        # The exact cleanup dependency keeps the rejected owner authenticated,
        # but ordinary account access is revoked immediately.
        cleanup = await client.get(
            "/owner-cleanup-proof",
            headers={"Authorization": f"Bearer {old_access_token}"},
        )
        assert cleanup.status_code == 200, cleanup.text
        revoked_access = await client.get(
            "/api/auth/me",
            headers={"Authorization": f"Bearer {old_access_token}"},
        )
        assert revoked_access.status_code == 401
        assert revoked_access.json()["detail"]["code"] == "AUTH_SESSION_REVOKED"

        async with factory() as session:
            locked_user = (
                await session.execute(
                    select(User).where(User.id == user_id).with_for_update()
                )
            ).scalar_one()
            locked_user.status = UserStatus.APPROVED
            await session.commit()

        still_revoked = await client.get(
            "/api/auth/me",
            headers={"Authorization": f"Bearer {old_access_token}"},
        )
        assert still_revoked.status_code == 401
        assert still_revoked.json()["detail"]["code"] == "AUTH_SESSION_REVOKED"
        cleanup_after_reapproval = await client.get(
            "/owner-cleanup-proof",
            headers={"Authorization": f"Bearer {old_access_token}"},
        )
        assert cleanup_after_reapproval.status_code == 401
        assert (
            cleanup_after_reapproval.json()["detail"]["code"]
            == "AUTH_SESSION_REVOKED"
        )

        old_refresh = await client.post("/api/auth/refresh")
        assert old_refresh.status_code == 401
        assert old_refresh.json()["detail"]["code"] == "REFRESH_SESSION_REVOKED"

    # No sleep or clock wait: issuance is explicitly later than the stored
    # microsecond cutoff even if both operations observe one clock tick.
    async with httpx.AsyncClient(
        transport=transport,
        base_url="https://test",
        headers={"Origin": "https://test"},
    ) as fresh_client:
        fresh_login = await fresh_client.post(
            "/api/auth/login",
            data={"username": user.email, "password": password},
        )
        assert fresh_login.status_code == 200, fresh_login.text
        fresh_access = fresh_login.json()["access_token"]
        cutoff_us = int(cutoff.timestamp()) * 1_000_000 + cutoff.microsecond
        assert decode_token(fresh_access)["iat_us"] > cutoff_us
        fresh_profile = await fresh_client.get(
            "/api/auth/me",
            headers={"Authorization": f"Bearer {fresh_access}"},
        )
        assert fresh_profile.status_code == 200, fresh_profile.text

    async with factory() as session:
        old_session = await session.scalar(
            select(RefreshSession).where(
                RefreshSession.user_id == user_id,
                RefreshSession.jti_hash == old_refresh_hash,
            )
        )
        assert old_session is not None
        assert old_session.revoked is True

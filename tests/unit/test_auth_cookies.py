"""
Unit tests for refresh-cookie migration behavior in auth_simple.
"""
from contextlib import asynccontextmanager
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

import pytest
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded

from app.database import get_db
from app.rate_limit import limiter
from app.models.refresh_session import RefreshSession
from app.routers import auth_simple
from app.routers.auth_simple import router
from app.models.user import UserRole, UserStatus


app = FastAPI()
app.state.limiter = limiter


@app.exception_handler(RateLimitExceeded)
async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
    return JSONResponse(
        status_code=429,
        content={"detail": f"Rate limit exceeded: {exc.detail}"},
    )


app.include_router(router, prefix="/api")


def _approved_user() -> SimpleNamespace:
    return SimpleNamespace(
        id=uuid4(),
        email="buyer@example.com",
        password_hash="stored-hash",
        first_name="Test",
        last_name="User",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        password_changed_at=None,
        organization_id=None,
    )


def _mock_db_session(user: SimpleNamespace) -> AsyncMock:
    session = AsyncMock()
    result = MagicMock()
    result.scalar_one_or_none.return_value = user
    session.execute = AsyncMock(return_value=result)
    session.commit = AsyncMock()
    session.refresh = AsyncMock()
    # record_login_day inspects the dialect for its upsert flavor.
    session.get_bind = MagicMock(
        return_value=SimpleNamespace(dialect=SimpleNamespace(name="sqlite"))
    )
    return session


@asynccontextmanager
async def _auth_client(session: AsyncMock):
    async def override_get_db():
        yield session

    app.dependency_overrides[get_db] = override_get_db
    try:
        async with AsyncClient(
            transport=ASGITransport(app=app),
            base_url="https://test",
            headers={"Origin": "https://test"},
        ) as client:
            yield client
    finally:
        app.dependency_overrides.pop(get_db, None)


def _cookie_header(response) -> str:
    return response.headers.get("set-cookie", "")


class TestRefreshCookieMigration:
    @pytest.mark.asyncio
    async def test_login_sets_http_only_refresh_cookie_and_preserves_body_token(self):
        user = _approved_user()
        session = _mock_db_session(user)

        with (
            patch("app.routers.auth_simple.verify_password_async", return_value=True),
            patch("app.routers.auth_simple.create_access_token", return_value="access-token-login"),
            patch("app.routers.auth_simple.create_refresh_token", return_value="refresh-token-login"),
            patch("app.routers.auth_simple._store_refresh_session", new=AsyncMock()),
        ):
            async with _auth_client(session) as client:
                response = await client.post(
                    "/api/auth/login",
                    data={"username": user.email, "password": "password"},
                )

        assert response.status_code == 200
        data = response.json()
        assert data["access_token"] == "access-token-login"
        assert data["profile"] == {
            "id": str(user.id),
            "email": user.email,
            "first_name": "Test",
            "last_name": "User",
            "role": "BUYER",
            "status": "APPROVED",
            "organization_id": None,
            "referral_code": None,
            "must_change_password": False,
        }
        assert "password_hash" not in response.text
        # The refresh token must travel ONLY in the HttpOnly cookie.
        assert "refresh_token" not in data
        cookie_header = _cookie_header(response)
        assert "refresh_token=refresh-token-login" in cookie_header
        assert "HttpOnly" in cookie_header
        assert "Path=/api/auth" in cookie_header

    @pytest.mark.asyncio
    async def test_login_sets_opaque_http_only_device_cookie_and_serializes_device_families(self, caplog):
        user = _approved_user()
        session = _mock_db_session(user)
        raw_device_id = "D" * 43
        device_hash = "a" * 64
        acquire_lock = AsyncMock()
        revoke_device = AsyncMock()

        with (
            patch("app.routers.auth_simple.verify_password_async", return_value=True),
            patch("app.routers.auth_simple.create_access_token", return_value="access-token-login"),
            patch("app.routers.auth_simple.create_refresh_token", return_value="refresh-token-login"),
            patch("app.routers.auth_simple._new_device_session_id", return_value=raw_device_id),
            patch("app.routers.auth_simple._device_session_hash", return_value=device_hash),
            patch("app.routers.auth_simple._acquire_device_session_lock", new=acquire_lock),
            patch("app.routers.auth_simple._revoke_device_refresh_sessions", new=revoke_device),
            patch("app.routers.auth_simple._store_refresh_session", new=AsyncMock()),
        ):
            async with _auth_client(session) as client:
                response = await client.post(
                    "/api/auth/login",
                    data={"username": user.email, "password": "password"},
                )

        assert response.status_code == 200
        assert raw_device_id not in response.text
        assert raw_device_id not in caplog.text
        assert "device_session" not in response.json()
        device_cookie = next(
            value
            for value in response.headers.get_list("set-cookie")
            if value.startswith(f"{auth_simple.DEVICE_SESSION_COOKIE_NAME}=")
        )
        assert f"{auth_simple.DEVICE_SESSION_COOKIE_NAME}={raw_device_id}" in device_cookie
        assert "HttpOnly" in device_cookie
        assert "Secure" in device_cookie
        assert "SameSite=lax" in device_cookie
        assert "Path=/api/auth" in device_cookie
        acquire_lock.assert_awaited_once_with(session, device_hash)
        revoke_device.assert_awaited_once()

    def test_refresh_session_persists_only_a_device_identifier_hash(self):
        assert "device_id_hash" in RefreshSession.__table__.columns
        column = RefreshSession.__table__.columns["device_id_hash"]
        assert column.type.length == 64
        # Legacy rows remain unknown and are revoked; they are never guessed/backfilled.
        assert column.nullable is True

    @pytest.mark.asyncio
    async def test_refresh_without_device_cookie_fails_closed_for_legacy_transition(self):
        user = _approved_user()
        session = _mock_db_session(user)
        token = auth_simple.create_refresh_token(str(user.id))

        async with _auth_client(session) as client:
            client.cookies.set(auth_simple.REFRESH_COOKIE_NAME, token, path="/api/auth")
            response = await client.post("/api/auth/refresh")

        assert response.status_code == 401
        assert response.json()["detail"]["code"] == "REFRESH_DEVICE_REQUIRED"

    @pytest.mark.asyncio
    async def test_refresh_uses_cookie_when_body_token_is_missing(self):
        user = _approved_user()
        session = _mock_db_session(user)

        access_tokens = iter(["access-token-login", "access-token-refresh"])
        refresh_tokens = iter(["refresh-token-login", "refresh-token-refresh"])

        def _decode_token(token: str):
            assert token == "refresh-token-login"
            return {"sub": str(user.id), "type": "refresh"}

        with (
            patch("app.routers.auth_simple.verify_password_async", return_value=True),
            patch("app.routers.auth_simple.create_access_token", side_effect=lambda *args, **kwargs: next(access_tokens)),
            patch("app.routers.auth_simple.create_refresh_token", side_effect=lambda *args, **kwargs: next(refresh_tokens)),
            patch("app.routers.auth_simple.decode_token", side_effect=_decode_token),
            patch("app.routers.auth_simple._store_refresh_session", new=AsyncMock()),
            patch("app.routers.auth_simple._rotate_refresh_session", new=AsyncMock()),
        ):
            async with _auth_client(session) as client:
                login_response = await client.post(
                    "/api/auth/login",
                    data={"username": user.email, "password": "password"},
                )
                assert login_response.status_code == 200

                refresh_response = await client.post("/api/auth/refresh")

        assert refresh_response.status_code == 200
        data = refresh_response.json()
        assert data["access_token"] == "access-token-refresh"
        assert "refresh_token" not in data
        assert client.cookies.get("refresh_token") == "refresh-token-refresh"

    @pytest.mark.asyncio
    async def test_refresh_still_accepts_body_token_for_compatibility(self):
        user = _approved_user()
        session = _mock_db_session(user)

        def _decode_token(token: str):
            assert token == "refresh-token-body"
            return {"sub": str(user.id), "type": "refresh"}

        with (
            patch("app.routers.auth_simple.create_access_token", return_value="access-token-refresh"),
            patch("app.routers.auth_simple.create_refresh_token", return_value="refresh-token-next"),
            patch("app.routers.auth_simple.decode_token", side_effect=_decode_token),
            patch("app.routers.auth_simple._rotate_refresh_session", new=AsyncMock()),
        ):
            async with _auth_client(session) as client:
                client.cookies.set(
                    auth_simple.DEVICE_SESSION_COOKIE_NAME,
                    "D" * 43,
                    path="/api/auth",
                )
                response = await client.post(
                    "/api/auth/refresh",
                    json={"refresh_token": "refresh-token-body"},
                )

        assert response.status_code == 200
        data = response.json()
        assert data["access_token"] == "access-token-refresh"
        assert "refresh_token" not in data

    @pytest.mark.asyncio
    async def test_logout_clears_refresh_cookie(self):
        user = _approved_user()
        session = _mock_db_session(user)

        with (
            patch("app.routers.auth_simple.verify_password_async", return_value=True),
            patch("app.routers.auth_simple.create_access_token", return_value="access-token-login"),
            patch("app.routers.auth_simple.create_refresh_token", return_value="refresh-token-login"),
            patch("app.routers.auth_simple.decode_token", return_value={"type": "refresh"}),
            patch("app.routers.auth_simple._store_refresh_session", new=AsyncMock()),
            patch("app.routers.auth_simple._revoke_refresh_family", new=AsyncMock()),
        ):
            async with _auth_client(session) as client:
                login_response = await client.post(
                    "/api/auth/login",
                    data={"username": user.email, "password": "password"},
                )
                assert login_response.status_code == 200
                assert client.cookies.get("refresh_token") == "refresh-token-login"

                logout_response = await client.post("/api/auth/logout")

        assert logout_response.status_code == 200
        assert client.cookies.get("refresh_token") is None
        assert client.cookies.get(auth_simple.DEVICE_SESSION_COOKIE_NAME) is None
        cookie_header = _cookie_header(logout_response).lower()
        assert "refresh_token=" in cookie_header
        assert "max-age=0" in cookie_header
        assert "path=/api/auth" in cookie_header

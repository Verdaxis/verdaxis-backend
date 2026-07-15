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
        async with AsyncClient(transport=ASGITransport(app=app), base_url="https://test") as client:
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
            patch("app.routers.auth_simple.verify_password", return_value=True),
            patch("app.routers.auth_simple.create_access_token", return_value="access-token-login"),
            patch("app.routers.auth_simple.create_refresh_token", return_value="refresh-token-login"),
        ):
            async with _auth_client(session) as client:
                response = await client.post(
                    "/api/auth/login",
                    data={"username": user.email, "password": "password"},
                )

        assert response.status_code == 200
        data = response.json()
        assert data["access_token"] == "access-token-login"
        # The refresh token must travel ONLY in the HttpOnly cookie.
        assert "refresh_token" not in data
        cookie_header = _cookie_header(response)
        assert "refresh_token=refresh-token-login" in cookie_header
        assert "HttpOnly" in cookie_header
        assert "Path=/api/auth" in cookie_header

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
            patch("app.routers.auth_simple.verify_password", return_value=True),
            patch("app.routers.auth_simple.create_access_token", side_effect=lambda *args, **kwargs: next(access_tokens)),
            patch("app.routers.auth_simple.create_refresh_token", side_effect=lambda *args, **kwargs: next(refresh_tokens)),
            patch("app.routers.auth_simple.decode_token", side_effect=_decode_token),
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
        ):
            async with _auth_client(session) as client:
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
            patch("app.routers.auth_simple.verify_password", return_value=True),
            patch("app.routers.auth_simple.create_access_token", return_value="access-token-login"),
            patch("app.routers.auth_simple.create_refresh_token", return_value="refresh-token-login"),
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
        cookie_header = _cookie_header(logout_response).lower()
        assert "refresh_token=" in cookie_header
        assert "max-age=0" in cookie_header
        assert "path=/api/auth" in cookie_header

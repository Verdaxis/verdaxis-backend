import asyncio
from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

import pytest
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded

from app.core import security
from app.core.security import create_access_token, decode_token
from app.database import get_db
from app.models.user import UserRole, UserStatus
from app.rate_limit import limiter
from app.routers import activity
from app.routers.auth_simple import router as auth_router


app = FastAPI()
app.state.limiter = limiter


@app.exception_handler(RateLimitExceeded)
async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
    return JSONResponse(
        status_code=429,
        content={"detail": f"Rate limit exceeded: {exc.detail}"},
    )


app.include_router(auth_router, prefix="/api")


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
        must_change_password=False,
        organization_id=uuid4(),
    )


def _mock_db_session(user: SimpleNamespace) -> AsyncMock:
    session = AsyncMock()
    user_result = MagicMock()
    user_result.scalar_one_or_none.return_value = user
    org_result = MagicMock()
    org_result.scalar_one_or_none.return_value = SimpleNamespace(
        id=user.organization_id,
        verification_status="APPROVED",
    )

    async def execute(statement):
        return org_result if "organizations" in str(statement) else user_result

    session.execute = AsyncMock(side_effect=execute)
    session.commit = AsyncMock()
    session.refresh = AsyncMock()
    return session


@asynccontextmanager
async def _short_session(user: SimpleNamespace):
    yield _mock_db_session(user)


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


class _RecordingEventBus:
    def __init__(self):
        self.subscribed: list[str] = []

    def subscribe(self, channel: str):
        self.subscribed.append(channel)
        return asyncio.Queue()

    def unsubscribe(self, channel: str, queue):
        pass


class _FakeRequest:
    headers = {}

    async def is_disconnected(self) -> bool:
        return True


class TestStreamTokenCreation:
    def test_create_stream_token_returns_single_purpose_short_lived_jwt(self):
        assert hasattr(security, "create_stream_token")

        user_id = uuid4()
        organization_id = uuid4()
        token = security.create_stream_token(user_id, organization_id)
        payload = decode_token(token)
        seconds_until_expiry = payload["exp"] - int(datetime.now(UTC).timestamp())

        assert payload["sub"] == str(user_id)
        assert payload["type"] == "stream"
        assert payload["org_id"] == str(organization_id)
        assert payload["environment"] == security.settings.ENVIRONMENT.strip().lower()
        assert 50 <= seconds_until_expiry <= 65
        assert "iat" in payload


class TestStreamTokenEndpoint:
    @pytest.mark.asyncio
    async def test_stream_token_endpoint_requires_authentication(self):
        async with _auth_client(_mock_db_session(_approved_user())) as client:
            response = await client.get("/api/auth/stream-token")

        assert response.status_code == 401

    @pytest.mark.asyncio
    async def test_stream_token_endpoint_returns_decodable_stream_token(self):
        user = _approved_user()
        session = _mock_db_session(user)
        access_token = create_access_token(str(user.id))

        async with _auth_client(session) as client:
            response = await client.get(
                "/api/auth/stream-token",
                headers={"Authorization": f"Bearer {access_token}"},
            )

        assert response.status_code == 200
        payload = decode_token(response.json()["stream_token"])
        seconds_until_expiry = payload["exp"] - int(datetime.now(UTC).timestamp())
        assert payload["sub"] == str(user.id)
        assert payload["type"] == "stream"
        assert payload["org_id"] == str(user.organization_id)
        assert 50 <= seconds_until_expiry <= 65

    @pytest.mark.asyncio
    async def test_stream_token_cannot_be_used_as_bearer_access_token(self):
        user = _approved_user()
        session = _mock_db_session(user)
        stream_token = create_access_token(
            str(user.id),
            additional_claims={"type": "stream"},
        )

        async with _auth_client(session) as client:
            response = await client.get(
                "/api/auth/me",
                headers={"Authorization": f"Bearer {stream_token}"},
            )

        assert response.status_code == 401


class TestActivityStreamQueryTokens:
    @pytest.mark.asyncio
    async def test_stream_token_in_query_resolves_user_for_org_channel(self):
        user = _approved_user()
        event_bus = _RecordingEventBus()
        stream_token = security.create_stream_token(user.id, user.organization_id)

        with patch.object(activity, "event_bus", event_bus), patch.object(
            activity, "AsyncSessionLocal", lambda: _short_session(user)
        ):
            response = await activity.stream_activity(
                request=_FakeRequest(),
                stream_token=stream_token,
            )
            with pytest.raises(StopAsyncIteration):
                await response.body_iterator.__anext__()

        assert response.status_code == 200
        assert event_bus.subscribed == [f"activity:{user.organization_id}"]

    @pytest.mark.asyncio
    async def test_access_token_in_query_is_rejected_for_org_channel(self):
        user = _approved_user()
        event_bus = _RecordingEventBus()
        access_token = create_access_token(str(user.id))

        with patch.object(activity, "event_bus", event_bus), patch.object(
            activity, "AsyncSessionLocal", lambda: _short_session(user)
        ):
            with pytest.raises(Exception) as exc_info:
                await activity.stream_activity(
                    request=_FakeRequest(),
                    stream_token=access_token,
                )

        assert exc_info.value.status_code == 401
        assert event_bus.subscribed == []

    @pytest.mark.asyncio
    async def test_expired_stream_token_in_query_is_rejected_for_org_channel(self):
        user = _approved_user()
        event_bus = _RecordingEventBus()
        expired_stream_token = create_access_token(
            str(user.id),
            expires_delta=timedelta(seconds=-1),
            additional_claims={"type": "stream"},
        )

        with patch.object(activity, "event_bus", event_bus), patch.object(
            activity, "AsyncSessionLocal", lambda: _short_session(user)
        ):
            with pytest.raises(Exception) as exc_info:
                await activity.stream_activity(
                    request=_FakeRequest(),
                    stream_token=expired_stream_token,
                )

        assert exc_info.value.status_code == 401
        assert event_bus.subscribed == []

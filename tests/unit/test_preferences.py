"""Unit tests for server-persisted user preferences."""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded

from app.database import get_db
from app.models.user_preference import UserPreference
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.routers.preferences import router as preferences_router


NOTIFICATION_PREFS = {
    "email_trade_updates": True,
    "email_market_alerts": False,
    "email_compliance_digest": True,
    "email_system_announcements": False,
    "inapp_trade_updates": True,
    "inapp_market_alerts": True,
    "inapp_order_matches": False,
}


class _ScalarRows:
    def __init__(self, rows):
        self._rows = rows

    def all(self):
        return list(self._rows)


class _ExecuteResult:
    def __init__(self, rows):
        self._rows = list(rows)

    def scalar_one_or_none(self):
        return self._rows[0] if self._rows else None

    def scalars(self):
        return _ScalarRows(self._rows)


class PreferenceSession:
    def __init__(self):
        self.preferences: dict[tuple[object, str], UserPreference] = {}
        self.execute_count = 0
        self.commit_count = 0

    async def execute(self, statement):
        self.execute_count += 1
        params = statement.compile().params
        user_id = params["user_id_1"]
        namespace = params.get("namespace_1")
        if namespace is not None:
            preference = self.preferences.get((user_id, namespace))
            return _ExecuteResult([preference] if preference else [])
        return _ExecuteResult(
            preference
            for (stored_user_id, _namespace), preference in self.preferences.items()
            if stored_user_id == user_id
        )

    def add(self, preference: UserPreference):
        self.preferences[(preference.user_id, preference.namespace)] = preference

    async def commit(self):
        self.commit_count += 1


def make_user():
    return SimpleNamespace(id=uuid4())


@asynccontextmanager
async def preference_client(session: PreferenceSession | None = None, user=None):
    app = FastAPI()
    app.state.limiter = limiter

    @app.exception_handler(RateLimitExceeded)
    async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
        return JSONResponse(
            status_code=429,
            content={"detail": f"Rate limit exceeded: {exc.detail}"},
        )

    if session is not None:
        async def override_get_db():
            yield session

        app.dependency_overrides[get_db] = override_get_db

    if user is not None:
        async def override_current_user():
            return user

        app.dependency_overrides[get_current_user] = override_current_user

    app.include_router(preferences_router, prefix="/api")

    async with AsyncClient(transport=ASGITransport(app=app), base_url="https://test") as client:
        yield client


@pytest.mark.asyncio
async def test_preferences_require_authentication_for_get_and_put():
    async with preference_client() as client:
        get_response = await client.get("/api/users/me/preferences")
        put_response = await client.put(
            "/api/users/me/preferences/tutorial",
            json={"completed": True},
        )

    assert get_response.status_code == 401
    assert put_response.status_code == 401


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("namespace", "payload"),
    [
        (
            "market_watch",
            {"products": ["BIO_METHANOL", "E_METHANOL"], "portIds": ["sg-sin", "nl-rtm"]},
        ),
        ("notifications", NOTIFICATION_PREFS),
        ("tutorial", {"completed": True}),
    ],
)
async def test_put_and_get_round_trip_supported_namespaces(namespace, payload):
    user = make_user()
    session = PreferenceSession()

    async with preference_client(session=session, user=user) as client:
        put_response = await client.put(f"/api/users/me/preferences/{namespace}", json=payload)
        get_response = await client.get("/api/users/me/preferences")

    assert put_response.status_code == 200
    assert put_response.json()["value"] == payload
    datetime.fromisoformat(put_response.json()["updated_at"])
    assert get_response.status_code == 200
    assert get_response.json() == {namespace: payload}
    assert session.execute_count == 2


@pytest.mark.asyncio
async def test_unknown_namespace_returns_404():
    user = make_user()
    session = PreferenceSession()

    async with preference_client(session=session, user=user) as client:
        response = await client.put("/api/users/me/preferences/unknown", json={"enabled": True})

    assert response.status_code == 404


@pytest.mark.asyncio
async def test_unknown_key_in_payload_returns_422():
    user = make_user()
    session = PreferenceSession()

    async with preference_client(session=session, user=user) as client:
        response = await client.put(
            "/api/users/me/preferences/tutorial",
            json={"completed": True, "extra": False},
        )

    assert response.status_code == 422


@pytest.mark.asyncio
async def test_oversize_payload_is_rejected():
    user = make_user()
    session = PreferenceSession()

    async with preference_client(session=session, user=user) as client:
        response = await client.put(
            "/api/users/me/preferences/market_watch",
            json={"products": ["BIO_METHANOL"], "portIds": ["x" * 9000]},
        )

    assert response.status_code == 413


@pytest.mark.asyncio
async def test_second_put_overwrites_and_bumps_updated_at():
    user = make_user()
    session = PreferenceSession()

    async with preference_client(session=session, user=user) as client:
        first_response = await client.put(
            "/api/users/me/preferences/tutorial",
            json={"completed": False},
        )
        await asyncio.sleep(0.001)
        second_response = await client.put(
            "/api/users/me/preferences/tutorial",
            json={"completed": True},
        )
        get_response = await client.get("/api/users/me/preferences")

    assert first_response.status_code == 200
    assert second_response.status_code == 200
    first_updated_at = datetime.fromisoformat(first_response.json()["updated_at"])
    second_updated_at = datetime.fromisoformat(second_response.json()["updated_at"])
    assert second_updated_at > first_updated_at
    assert second_response.json()["value"] == {"completed": True}
    assert get_response.json() == {"tutorial": {"completed": True}}


@pytest.mark.asyncio
async def test_users_cannot_read_other_users_preferences():
    user_a = make_user()
    user_b = make_user()
    session = PreferenceSession()

    async with preference_client(session=session, user=user_a) as client:
        response = await client.put(
            "/api/users/me/preferences/tutorial",
            json={"completed": True},
        )
    assert response.status_code == 200

    async with preference_client(session=session, user=user_b) as client:
        response = await client.get("/api/users/me/preferences")

    assert response.status_code == 200
    assert response.json() == {}


def test_migration_declares_user_preferences_fk_ondelete_cascade():
    migration = Path(__file__).parents[2] / "alembic/versions/pref_20260709_add_user_preferences.py"
    contents = migration.read_text()

    assert '"user_preferences"' in contents
    assert '["user_id"], ["users.id"], ondelete="CASCADE"' in contents

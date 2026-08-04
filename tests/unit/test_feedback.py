"""Unit tests for in-app feedback and the admin onboarding-attention view."""
from __future__ import annotations

from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded

from app.database import get_db
from app.models.user import UserRole
from app.rate_limit import limiter
from app.routers.admin_attention import router as admin_attention_router
from app.routers.auth_simple import get_current_user
from app.routers.feedback import router as feedback_router
from app.services.onboarding_attention import OnboardingCandidate


class _Count:
    def __init__(self, value: int):
        self._value = value

    def scalar(self):
        return self._value


class _Rows:
    def __init__(self, rows):
        self._rows = list(rows)

    def __iter__(self):
        return iter(self._rows)


class FeedbackSession:
    """Stores added entries; serves them back joined to a fixed user/org."""

    def __init__(self):
        self.entries = []
        self.commit_count = 0

    def add(self, entry):
        self.entries.append(entry)

    async def commit(self):
        self.commit_count += 1

    async def execute(self, statement):
        if "count" in str(statement).lower():
            return _Count(len(self.entries))
        ordered = sorted(self.entries, key=lambda e: e.created_at, reverse=True)
        return _Rows(
            (entry, "ada@example.com", "Ada", "Lovelace", "Acme Shipping")
            for entry in ordered
        )


def make_user(role: UserRole = UserRole.BUYER):
    return SimpleNamespace(id=uuid4(), role=role)


@pytest.fixture(autouse=True)
def _reset_rate_limiter():
    # POSTs are limited 5/minute per client and every test shares the ASGI
    # test client address; without a reset the suite trips its own limit.
    limiter.reset()
    yield


@asynccontextmanager
async def feedback_client(session=None, user=None):
    app = FastAPI()
    app.state.limiter = limiter

    @app.exception_handler(RateLimitExceeded)
    async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
        return JSONResponse(status_code=429, content={"detail": "Rate limit exceeded"})

    if session is not None:
        async def override_get_db():
            yield session

        app.dependency_overrides[get_db] = override_get_db

    if user is not None:
        async def override_current_user():
            return user

        app.dependency_overrides[get_current_user] = override_current_user

    app.include_router(feedback_router, prefix="/api")
    app.include_router(admin_attention_router, prefix="/api")

    async with AsyncClient(transport=ASGITransport(app=app), base_url="https://test") as client:
        yield client


@pytest.mark.asyncio
async def test_feedback_endpoints_require_authentication():
    async with feedback_client() as client:
        post_response = await client.post("/api/feedback", json={"message": "hello"})
        get_response = await client.get("/api/admin/feedback")

    assert post_response.status_code == 401
    assert get_response.status_code == 401


@pytest.mark.asyncio
async def test_submit_persists_trimmed_message_and_page():
    session = FeedbackSession()
    async with feedback_client(session=session, user=make_user()) as client:
        response = await client.post(
            "/api/feedback",
            json={"message": "  the order form is confusing  ", "page": "/app/marketplace"},
        )

    assert response.status_code == 201
    body = response.json()
    datetime.fromisoformat(body["created_at"])
    assert session.commit_count == 1
    (entry,) = session.entries
    assert entry.message == "the order form is confusing"
    assert entry.page == "/app/marketplace"


@pytest.mark.asyncio
@pytest.mark.parametrize("message", ["", "   ", "x" * 2001])
async def test_blank_or_oversize_message_rejected(message):
    session = FeedbackSession()
    async with feedback_client(session=session, user=make_user()) as client:
        response = await client.post("/api/feedback", json={"message": message})

    assert response.status_code == 422
    assert session.entries == []


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "page",
    ["https://evil.example/x", "/app?token=secret", "/app#frag", "//evil.example", "app/relative"],
)
async def test_non_path_page_metadata_is_dropped_not_rejected(page):
    session = FeedbackSession()
    async with feedback_client(session=session, user=make_user()) as client:
        response = await client.post("/api/feedback", json={"message": "hi", "page": page})

    assert response.status_code == 201
    (entry,) = session.entries
    assert entry.page is None


@pytest.mark.asyncio
async def test_admin_feedback_list_requires_admin_role():
    session = FeedbackSession()
    async with feedback_client(session=session, user=make_user(UserRole.BUYER)) as client:
        response = await client.get("/api/admin/feedback")

    assert response.status_code == 403


@pytest.mark.asyncio
async def test_admin_feedback_list_returns_identified_entries():
    session = FeedbackSession()
    async with feedback_client(session=session, user=make_user()) as client:
        await client.post("/api/feedback", json={"message": "first"})
        await client.post("/api/feedback", json={"message": "second"})

    async with feedback_client(session=session, user=make_user(UserRole.ADMIN)) as client:
        response = await client.get("/api/admin/feedback")

    assert response.status_code == 200
    body = response.json()
    assert body["total"] == 2
    assert [item["message"] for item in body["items"]] == ["second", "first"]
    assert body["items"][0]["user_email"] == "ada@example.com"
    assert body["items"][0]["user_name"] == "Ada Lovelace"
    assert body["items"][0]["org_name"] == "Acme Shipping"


def _candidate(**overrides) -> OnboardingCandidate:
    now = datetime.now(UTC)
    values = dict(
        key="user:1",
        email="stalled@example.com",
        first_name="Stal",
        last_name="Led",
        role="BUYER",
        created_at=now - timedelta(days=2),
        email_verified=True,
        verification_expires_at=None,
        user_status="APPROVED",
        organization_name="Acme Shipping",
        organization_status="APPROVED",
        organization_provenance="REAL",
        membership_status="APPROVED",
        membership_reviewed_at=now - timedelta(hours=3),
        user_approved_at=now - timedelta(hours=3),
        organization_approved_at=now - timedelta(hours=3),
        last_login=None,
        pending_registration_expires_at=None,
    )
    values.update(overrides)
    return OnboardingCandidate(**values)


@pytest.mark.asyncio
async def test_onboarding_attention_lists_stalled_users_only(monkeypatch):
    stalled = _candidate()
    healthy = _candidate(key="user:2", email="fine@example.com", last_login=datetime.now(UTC))

    async def fake_load(db):
        return [stalled, healthy]

    monkeypatch.setattr("app.routers.admin_attention.load_candidates", fake_load)

    async with feedback_client(session=FeedbackSession(), user=make_user(UserRole.ADMIN)) as client:
        response = await client.get("/api/admin/onboarding-attention")

    assert response.status_code == 200
    body = response.json()
    assert [item["email"] for item in body["items"]] == ["stalled@example.com"]
    assert body["items"][0]["stage"] == "first_login_overdue"
    assert body["items"][0]["organization_name"] == "Acme Shipping"


@pytest.mark.asyncio
async def test_onboarding_attention_requires_admin_role(monkeypatch):
    async def fake_load(db):  # pragma: no cover - must not be reached
        raise AssertionError("load_candidates must not run for non-admins")

    monkeypatch.setattr("app.routers.admin_attention.load_candidates", fake_load)

    async with feedback_client(session=FeedbackSession(), user=make_user(UserRole.SUPPLIER)) as client:
        response = await client.get("/api/admin/onboarding-attention")

    assert response.status_code == 403


def test_migration_declares_feedback_fk_ondelete_cascade():
    migration = Path(__file__).parents[2] / "alembic/versions/fb_20260804_add_feedback_entries.py"
    contents = migration.read_text()

    assert '"feedback_entries"' in contents
    assert '["user_id"], ["users.id"], ondelete="CASCADE"' in contents

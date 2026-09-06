from datetime import datetime, UTC
from types import SimpleNamespace
from uuid import uuid4

import pytest
from httpx import ASGITransport, AsyncClient

from app.database import get_db
from app.main import app
from app.routers.auth_simple import get_current_user
from app.schemas.watchlist import WatchlistSummaryResponse


class _RouteDb:
    async def commit(self):
        return None


@pytest.fixture
def route_dependencies():
    user = SimpleNamespace(
        id=uuid4(),
        email_verified=True,
        referral_code="VDX-ABC123",
    )
    db = _RouteDb()

    async def override_user():
        return user

    async def override_db():
        yield db

    app.dependency_overrides[get_current_user] = override_user
    app.dependency_overrides[get_db] = override_db
    yield user
    app.dependency_overrides.pop(get_current_user, None)
    app.dependency_overrides.pop(get_db, None)


@pytest.mark.asyncio
async def test_state_changing_read_routes_accept_post_and_deny_get(
    route_dependencies, monkeypatch
):
    radar = SimpleNamespace(id=uuid4())
    summary = WatchlistSummaryResponse(
        id=radar.id,
        name="Market Radar",
        kind="RADAR_DEFAULT",
        created_at=datetime.now(UTC),
    )

    async def ensure_radar(db, user_id):
        return radar

    async def load_radar(db, watchlist_id, user_id):
        return radar

    async def build_summary(db, watchlist):
        return summary

    monkeypatch.setattr("app.routers.watchlists.ensure_market_radar", ensure_radar)
    monkeypatch.setattr("app.routers.watchlists.load_watchlist_or_404", load_radar)
    monkeypatch.setattr("app.routers.watchlists.build_watchlist_summary", build_summary)

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        watchlist_post = await client.post("/api/watchlists/me")
        referral_post = await client.post("/api/referrals/my-code")
        watchlist_get = await client.get("/api/watchlists/me")
        referral_get = await client.get("/api/referrals/my-code")

    assert watchlist_post.status_code == 200
    assert referral_post.status_code == 200
    assert watchlist_get.status_code == 405
    assert referral_get.status_code == 405


@pytest.mark.asyncio
async def test_state_changing_read_routes_reject_market_support_context(
    route_dependencies, monkeypatch
):
    async def fail_if_called(*args, **kwargs):
        raise AssertionError("support-scoped route must be denied by middleware")

    monkeypatch.setattr("app.routers.watchlists.ensure_market_radar", fail_if_called)

    headers = {"X-Verdaxis-Market-Support-Context": str(uuid4())}
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        watchlist_post = await client.post("/api/watchlists/me", headers=headers)
        referral_post = await client.post("/api/referrals/my-code", headers=headers)

    assert watchlist_post.status_code == 403
    assert referral_post.status_code == 403

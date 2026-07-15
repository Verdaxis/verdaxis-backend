"""Endpoint tests for the seven Product Analytics tabs (plan Task 6)."""

from __future__ import annotations

from datetime import UTC, datetime

import pytest
from fastapi import FastAPI
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded
from slowapi.middleware import SlowAPIMiddleware
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from uuid import uuid4

from app.database import Base, get_db
from app.main import app as main_app
from app.models.user import User, UserRole, UserStatus
from app.rate_limit import limiter
from app.routers.admin_analytics import router
from app.routers.auth_simple import get_current_user
from app.services.behavioral_analytics import (
    AnalyticsDiagnostic,
    BehavioralWindowAggregate,
    get_analytics_service,
)
from tests.unit.fixtures import product_analytics as fixtures

TABS = (
    "overview", "acquisition", "activation", "engagement",
    "marketplace", "retention", "reliability",
)
# "+00:00" offsets decode as spaces inside query strings; Z is unambiguous.
_BASE_QS = "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z"


def _user(role: UserRole) -> User:
    return User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=role,
        status=UserStatus.APPROVED,
    )


def _window(status: str = "available") -> BehavioralWindowAggregate:
    if status == "unavailable":
        return BehavioralWindowAggregate.unavailable(
            AnalyticsDiagnostic.TIMEOUT, fixtures.PERIOD_START, fixtures.PERIOD_END
        )
    return BehavioralWindowAggregate(
        status=status,
        diagnostic=None,
        observed_at=datetime.now(UTC),
        start=fixtures.PERIOD_START,
        end=fixtures.PERIOD_END,
        visitors=120,
        visits=90,
        pageviews=300,
        total_time_seconds=1800,
        event_totals={"signup_started": 10, "landing_cta_clicked": 25, "login_failed": 6},
        event_series=[{"date": "2026-06-10", "event": "login_failed", "value": 6}],
        daily_visitors=[{"date": "2026-06-10", "value": 30}],
        top_entries=[{"name": "/signup", "value": 40}],
        top_referrers=[{"name": "", "value": 10}, {"name": "google.com", "value": 25}],
        event_properties={
            "landing_cta_clicked": [
                {"property": "cta", "value": "pilot", "total": 12},
                {"property": "placement", "value": "hero", "total": 9},
            ],
            "platform_navigation": [
                {"property": "destination", "value": "marketplace", "total": 18},
            ],
            "navigation_performance": [
                {"property": "latency_bucket", "value": "lt250", "total": 14},
            ],
            "frontend_error": [
                {"property": "category", "value": "render", "total": 2},
            ],
        },
    )


class _StubAnalytics:
    def __init__(self, status: str = "available"):
        self.status = status

    async def get_window_aggregate(self, start, end, *, event_properties=()):
        return _window(self.status)


@pytest.fixture(autouse=True)
def _rate_limits_off(monkeypatch):
    monkeypatch.setattr(limiter, "enabled", False)


@pytest.fixture
async def seeded_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    from app.models.audit import AuditLog

    tables = list(fixtures.scenario_tables()) + [AuditLog.__table__]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        await fixtures.seed_product_analytics_scenario(session)
        await fixtures.seed_fact_tables(session)
        yield session
    await engine.dispose()


def _app(db, current_user: User, analytics=None) -> FastAPI:
    app = FastAPI()
    app.state.limiter = limiter
    app.add_exception_handler(
        RateLimitExceeded,
        lambda _r, exc: JSONResponse(status_code=429, content={"detail": str(exc.detail)}),
    )
    app.add_middleware(SlowAPIMiddleware)
    app.include_router(router, prefix="/api")

    async def override_db():
        yield db

    app.dependency_overrides[get_db] = override_db
    app.dependency_overrides[get_current_user] = lambda: current_user
    app.dependency_overrides[get_analytics_service] = lambda: analytics or _StubAnalytics()
    return app


async def _get(app: FastAPI, path: str, headers: dict | None = None):
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        return await client.get(path, headers=headers)


@pytest.mark.parametrize("tab", TABS)
async def test_every_tab_requires_admin(tab, seeded_db):
    app = _app(seeded_db, _user(UserRole.BUYER))
    response = await _get(app, f"/api/admin/analytics/product-analytics/{tab}?{_BASE_QS}")
    assert response.status_code == 403


@pytest.mark.parametrize(
    "qs",
    [
        "start=2026-07-01T00:00:00Z&end=2026-06-01T00:00:00Z",
        "start=2025-01-01T00:00:00Z&end=2026-06-01T00:00:00Z",  # > 365 days
        f"{_BASE_QS}&audience=EVERYONE",
    ],
)
async def test_invalid_queries_are_rejected(qs, seeded_db):
    app = _app(seeded_db, _user(UserRole.ADMIN))
    response = await _get(app, f"/api/admin/analytics/product-analytics/overview?{qs}")
    assert response.status_code == 422


@pytest.mark.parametrize("tab", [t for t in TABS if t != "marketplace"])
async def test_reference_activity_is_marketplace_only(tab, seeded_db):
    app = _app(seeded_db, _user(UserRole.ADMIN))
    response = await _get(
        app, f"/api/admin/analytics/product-analytics/{tab}?{_BASE_QS}&activity=REFERENCE"
    )
    assert response.status_code == 422


async def test_overview_composes_authoritative_and_behavioral(seeded_db):
    app = _app(seeded_db, _user(UserRole.ADMIN))
    response = await _get(app, f"/api/admin/analytics/product-analytics/overview?{_BASE_QS}")
    assert response.status_code == 200, response.text
    body = response.json()

    assert body["kpis"]["live_orders"]["value"] == fixtures.EXPECTED["orders"]["live_current"]
    assert body["kpis"]["qualified_organizations"]["value"] == 3
    stages = {stage["key"]: stage for stage in body["lifecycle"]}
    assert stages["visitors"]["count"] == 120
    assert stages["registered"]["count"] == 2
    # No cross-stage conversion percentages on the spine (§1.6).
    assert "conversion" not in str(body["lifecycle"]).lower()
    assert body["meta"]["coverage"]["behavioral"]["status"] == "available"
    assert body["meta"]["coverage"]["login_history"]["status"] == "partial"


async def test_degraded_behavioral_keeps_authoritative_data(seeded_db):
    app = _app(seeded_db, _user(UserRole.ADMIN), analytics=_StubAnalytics("unavailable"))
    response = await _get(app, f"/api/admin/analytics/product-analytics/overview?{_BASE_QS}")
    assert response.status_code == 200
    body = response.json()
    assert body["kpis"]["live_orders"]["value"] == fixtures.EXPECTED["orders"]["live_current"]
    assert body["meta"]["coverage"]["behavioral"]["status"] == "unavailable"
    stages = {stage["key"]: stage for stage in body["lifecycle"]}
    assert stages["visitors"]["count"] is None
    rules = {item["rule"] for item in body["needs_attention"]}
    assert "degraded_analytics_collection" in rules


async def test_marketplace_reference_and_filter_validation(seeded_db):
    app = _app(seeded_db, _user(UserRole.ADMIN))

    reference = await _get(
        app, f"/api/admin/analytics/product-analytics/marketplace?{_BASE_QS}&activity=REFERENCE"
    )
    assert reference.status_code == 200
    body = reference.json()
    assert body["live"] is None
    assert len(body["reference"]["rows"]) == 4
    assert body["meta"]["coverage"]["reference"]["status"] == "available"

    bogus = await _get(
        app,
        f"/api/admin/analytics/product-analytics/marketplace?{_BASE_QS}&product_id={uuid4()}",
    )
    assert bogus.status_code == 422

    bad_window = await _get(
        app,
        f"/api/admin/analytics/product-analytics/marketplace?{_BASE_QS}&availability_window=SOON",
    )
    assert bad_window.status_code == 422

    filtered = await _get(
        app,
        "/api/admin/analytics/product-analytics/marketplace"
        f"?{_BASE_QS}&product_id={fixtures.PRODUCT_BIO_METHANOL_ID}"
        f"&delivery_point_id={fixtures.DELIVERY_POINT_ROTTERDAM_ID}",
    )
    assert filtered.status_code == 200
    live = filtered.json()["live"]
    # Only the Rotterdam ask exists in that slice.
    assert live["kpis"]["open_asks"]["value"] == 1
    assert live["kpis"]["open_bids"]["value"] == 0


async def test_suppressed_cells_and_no_pii_in_responses(seeded_db):
    app = _app(seeded_db, _user(UserRole.ADMIN))
    for tab in TABS:
        qs = _BASE_QS + ("&activity=ALL" if tab == "marketplace" else "")
        response = await _get(app, f"/api/admin/analytics/product-analytics/{tab}?{qs}")
        assert response.status_code == 200, (tab, response.text)
        raw = response.text.lower()
        for forbidden in (
            "@fixtures.verdaxis.test", "live buyer shipping", "live supplier fuels",
            "returning trader", "ip_address",
        ):
            assert forbidden not in raw, (tab, forbidden)
        assert len(response.content) <= 250_000, (tab, len(response.content))

    activation = await _get(app, f"/api/admin/analytics/product-analytics/activation?{_BASE_QS}")
    drop_off = {cell["key"]: cell for cell in activation.json()["drop_off"]}
    assert drop_off["rejected"]["suppressed"] is True
    assert drop_off["rejected"]["count"] is None


async def test_mixed_coverage_states_without_fact_rows():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    from app.models.audit import AuditLog

    tables = list(fixtures.scenario_tables()) + [AuditLog.__table__]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    try:
        async with factory() as session:
            await fixtures.seed_product_analytics_scenario(session)
            app = _app(session, _user(UserRole.ADMIN))
            response = await _get(
                app, f"/api/admin/analytics/product-analytics/overview?{_BASE_QS}"
            )
            body = response.json()
            # Authoritative data present while fact sources report their own
            # insufficient coverage — no healthy source masks another's gap.
            assert body["kpis"]["live_orders"]["value"] > 0
            assert body["kpis"]["active_members"]["value"] is None
            assert body["meta"]["coverage"]["login_history"]["status"] == "unavailable"
            assert (
                body["meta"]["coverage"]["login_history"]["diagnostic"]
                == "insufficient_coverage"
            )
            assert body["meta"]["coverage"]["authoritative"]["status"] == "available"
    finally:
        await engine.dispose()


async def test_rate_limit_applies_per_token(seeded_db, monkeypatch):
    monkeypatch.setattr(limiter, "enabled", True)
    app = _app(seeded_db, _user(UserRole.ADMIN), analytics=_StubAnalytics("unavailable"))
    headers = {"Authorization": f"Bearer rate-limit-test-{uuid4()}"}
    statuses = []
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        for _ in range(31):
            response = await client.get(
                f"/api/admin/analytics/product-analytics/retention?{_BASE_QS}",
                headers=headers,
            )
            statuses.append(response.status_code)
    assert statuses[:30] == [200] * 30
    assert statuses[30] == 429


def test_openapi_contract_exposes_all_seven_tabs():
    schema = main_app.openapi()
    for tab in TABS:
        path = f"/api/admin/analytics/product-analytics/{tab}"
        assert path in schema["paths"], path
        operation = schema["paths"][path]["get"]
        ref = operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"]
        assert ref.startswith("#/components/schemas/"), (path, ref)
    assert "AnalyticsMeta" in schema["components"]["schemas"]
    assert "AggregateCell" in schema["components"]["schemas"]

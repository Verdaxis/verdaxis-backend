from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded
from slowapi.middleware import SlowAPIMiddleware
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base, get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.rate_limit import limiter
from app.main import app as main_app
from app.routers.admin_analytics import router
from app.routers.auth_simple import get_current_user
from app.services.behavioral_analytics import (
    AnalyticsDiagnostic,
    BehavioralAggregate,
    get_analytics_service,
)
from app.services.demo_market import DEMO_ACTIVITY_BUYER_ORG_ID


@pytest.fixture
async def analytics_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = [
        Organization.__table__, User.__table__, Product.__table__,
        DeliveryPoint.__table__, OrderBookOrder.__table__,
    ]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


def _user(role: UserRole, org_id=None, **values):
    return User(
        id=values.get("id", uuid4()),
        email=values.get("email", f"{uuid4()}@example.test"),
        password_hash="hash",
        role=role,
        status=UserStatus.APPROVED,
        organization_id=org_id,
        created_at=values.get("created_at", datetime.now(UTC)),
        last_login=values.get("last_login"),
    )


async def _seed_counts(db: AsyncSession):
    now = datetime.now(UTC)
    real_org = Organization(id=uuid4(), name="Real Org", type=OrgType.FUEL_BUYER)
    inactive_org = Organization(id=uuid4(), name="Inactive Org", type=OrgType.FUEL_SUPPLIER)
    demo_org = Organization(id=DEMO_ACTIVITY_BUYER_ORG_ID, name="Demo", type=OrgType.FUEL_BUYER)
    product = Product(id=uuid4(), name="Bio Methanol", fuel_type="Methanol", fuel_grade="Bio")
    point = DeliveryPoint(id=uuid4(), name="Singapore", region="Asia")
    db.add_all([real_org, inactive_org, demo_org, product, point])
    db.add_all([
        _user(UserRole.BUYER, real_org.id, created_at=now - timedelta(days=2), last_login=now - timedelta(days=1)),
        _user(UserRole.SUPPLIER, inactive_org.id, created_at=now - timedelta(days=40), last_login=now - timedelta(days=40)),
        _user(UserRole.BUYER, demo_org.id, created_at=now - timedelta(days=1), last_login=now - timedelta(days=1)),
        _user(UserRole.ADMIN, None, created_at=now - timedelta(days=1), last_login=now - timedelta(days=1)),
    ])
    db.add_all([
        OrderBookOrder(
            organization_id=real_org.id, side=OrderSide.BID, product_id=product.id,
            delivery_point_id=point.id, quantity_mt=Decimal("100"),
            remaining_quantity_mt=Decimal("100"), price_per_mt_usd=Decimal("500"),
            availability_window="SPOT", status=OrderBookStatus.OPEN,
            created_at=now - timedelta(days=1),
        ),
        OrderBookOrder(
            organization_id=demo_org.id, side=OrderSide.BID, product_id=product.id,
            delivery_point_id=point.id, quantity_mt=Decimal("100"),
            remaining_quantity_mt=Decimal("100"), price_per_mt_usd=Decimal("500"),
            availability_window="SPOT", status=OrderBookStatus.OPEN,
            created_at=now - timedelta(days=1),
        ),
    ])
    await db.commit()


def _app(db, current_user, aggregate):
    app = FastAPI()
    app.state.limiter = limiter
    app.add_exception_handler(
        RateLimitExceeded,
        lambda _request, _exc: None,
    )
    app.add_middleware(SlowAPIMiddleware)
    app.include_router(router, prefix="/api")

    async def override_db():
        yield db

    async def override_user():
        return current_user

    class StubService:
        async def get_aggregate(self, _days):
            return aggregate

    app.dependency_overrides[get_db] = override_db
    app.dependency_overrides[get_current_user] = override_user
    app.dependency_overrides[get_analytics_service] = lambda: StubService()
    return app


@pytest.mark.asyncio
async def test_product_usage_is_admin_only(analytics_db):
    app = _app(
        analytics_db,
        _user(UserRole.BUYER),
        BehavioralAggregate.unavailable(AnalyticsDiagnostic.DISABLED),
    )
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get("/api/admin/analytics/product-usage?days=7")
    assert response.status_code == 403


@pytest.mark.asyncio
@pytest.mark.parametrize("days", [1, 8, 31, 365])
async def test_product_usage_accepts_only_7_30_90(days, analytics_db):
    app = _app(
        analytics_db,
        _user(UserRole.ADMIN),
        BehavioralAggregate.unavailable(AnalyticsDiagnostic.DISABLED),
    )
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get(f"/api/admin/analytics/product-usage?days={days}")
    assert response.status_code == 422


@pytest.mark.asyncio
async def test_unavailable_umami_still_returns_authoritative_non_demo_db_counts(analytics_db):
    await _seed_counts(analytics_db)
    aggregate = BehavioralAggregate.unavailable(AnalyticsDiagnostic.TIMEOUT)
    app = _app(analytics_db, _user(UserRole.ADMIN), aggregate)

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get("/api/admin/analytics/product-usage?days=7")

    assert response.status_code == 200, response.text
    body = response.json()
    assert body["behavioral_status"] == "unavailable"
    assert body["diagnostic"] == "timeout"
    assert body["authoritative"] == {
        "registrations": 2,
        "users_logging_in": 2,
        "order_placing_organizations": 1,
    }
    serialized = response.text.lower()
    for forbidden in ["real org", "@example.test", "price", "quantity", "order_id", "trade_id"]:
        assert forbidden not in serialized


@pytest.mark.asyncio
async def test_available_response_exposes_only_aggregates_and_calculated_duration(analytics_db):
    aggregate = BehavioralAggregate(
        status="available",
        diagnostic=None,
        observed_at=datetime.now(UTC),
        visitors=10,
        visits=5,
        pageviews=20,
        total_time_seconds=100,
        event_totals={"signup_started": 4},
        event_series=[{"date": "2026-07-13", "event": "signup_started", "value": 2}],
        daily_visitors=[{"date": "2026-07-13", "value": 3}],
        top_entries=[{"name": "/signup", "value": 4}],
        top_referrers=[{"name": "google.com", "value": 2}],
    )
    app = _app(analytics_db, _user(UserRole.ADMIN), aggregate)

    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get("/api/admin/analytics/product-usage?days=30")

    body = response.json()
    assert response.status_code == 200, response.text
    assert body["behavioral"]["average_session_duration_seconds"] == 20
    assert "average_duration_seconds" not in body["behavioral"]
    assert body["behavioral"]["event_totals"] == {"signup_started": 4}
    assert body["behavioral"]["event_series"] == [
        {"date": "2026-07-13", "event": "signup_started", "value": 2}
    ]
    forbidden_keys = {"sessions", "distinct_ids", "email", "organization_names", "prices", "quantities"}
    assert forbidden_keys.isdisjoint(body)


def test_product_usage_openapi_contract_is_additive_and_typed():
    schema = main_app.openapi()
    operation = schema["paths"]["/api/admin/analytics/product-usage"]["get"]
    period_schema = schema["components"]["schemas"]["ProductUsagePeriod"]
    response_schema = schema["components"]["schemas"]["ProductUsageResponse"]
    behavioral_schema = schema["components"]["schemas"]["BehavioralUsage"]

    assert period_schema["enum"] == [7, 30, 90]
    assert operation["responses"]["200"]["content"]["application/json"]["schema"] == {
        "$ref": "#/components/schemas/ProductUsageResponse"
    }
    assert set(response_schema["required"]) == {
        "days", "period_start", "period_end", "behavioral_status", "diagnostic",
        "observed_at", "behavioral", "authoritative", "funnel",
    }
    assert "average_session_duration_seconds" in behavioral_schema["properties"]
    assert "average_duration_seconds" not in behavioral_schema["properties"]

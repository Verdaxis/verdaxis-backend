"""Security, idempotency, and pagination checks for per-user activity."""

from __future__ import annotations

from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import get_db
from app.model_base import Base
from app.models.user import Organization, User, UserRole, UserStatus
from app.models.audit import AuditLog
from app.models.product_analytics import UserLoginDay
from app.models.user_activity import UserBrowsingEvent
from app.models.user_preference import UserPreference
from app.models.watchlist import Watchlist, WatchlistTarget, WatchlistTargetType
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.routers.user_activity import MAX_ACTIVITY_BODY_BYTES, router
from app.schemas.user_activity import BrowsingEventsIn
from app.services.user_activity import get_user_activity_page, ingest_browsing_events


@pytest.fixture
async def activity_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as connection:
        await connection.run_sync(
            Base.metadata.create_all,
            tables=[
                Organization.__table__,
                User.__table__,
                AuditLog.__table__,
                UserLoginDay.__table__,
                UserBrowsingEvent.__table__,
                UserPreference.__table__,
                Watchlist.__table__,
                WatchlistTarget.__table__,
            ],
        )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


def _user(role: UserRole = UserRole.BUYER) -> SimpleNamespace:
    return SimpleNamespace(id=uuid4(), role=role, status=UserStatus.APPROVED)


@asynccontextmanager
async def _client(*, db=None, user=None):
    app = FastAPI()
    app.state.limiter = limiter

    @app.exception_handler(RateLimitExceeded)
    async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
        return JSONResponse(status_code=429, content={"detail": str(exc.detail)})

    if db is not None:
        async def override_db():
            yield db

        app.dependency_overrides[get_db] = override_db
    if user is not None:
        async def override_user():
            return user

        app.dependency_overrides[get_current_user] = override_user
    app.include_router(router, prefix="/api")
    async with AsyncClient(
        transport=ASGITransport(app=app),
        base_url="https://test",
    ) as client:
        yield client


def _payload(event_id, *, page="marketplace") -> BrowsingEventsIn:
    return BrowsingEventsIn.model_validate(
        {
            "events": [{"id": str(event_id), "action": "page_view", "page": page}],
        }
    )


async def test_ingestion_requires_authentication_and_rejects_identity_or_time_fields():
    event_id = uuid4()
    async with _client() as client:
        unauthenticated = await client.post(
            "/api/activity/events",
            json=_payload(event_id).model_dump(mode="json"),
        )
    assert unauthenticated.status_code == 401

    async with _client(user=_user()) as client:
        spoofed = await client.post(
            "/api/activity/events",
            json={
                "user_id": str(uuid4()),
                "events": [
                    {
                        "id": str(event_id),
                        "action": "page_view",
                        "page": "marketplace",
                        "occurred_at": datetime.now(UTC).isoformat(),
                    }
                ],
            },
        )
    assert spoofed.status_code == 422


async def test_activity_body_limit_rejects_declared_and_chunked_oversize_before_auth():
    async with _client() as client:
        declared = await client.post(
            "/api/activity/events",
            content=b"{}",
            headers={"Content-Length": str(MAX_ACTIVITY_BODY_BYTES + 1)},
        )

        async def chunks():
            yield b"{" + (b"x" * MAX_ACTIVITY_BODY_BYTES)
            yield b"x}"

        chunked = await client.post("/api/activity/events", content=chunks())

    assert declared.status_code == 413
    assert chunked.status_code == 413


async def test_two_tokens_for_one_user_share_the_write_rate_limit(activity_db):
    user = _user()
    event = _payload(uuid4()).model_dump(mode="json")
    async with _client(db=activity_db, user=user) as client:
        responses = []
        for index in range(61):
            responses.append(
                await client.post(
                    "/api/activity/events",
                    json=event,
                    headers={"Authorization": f"Bearer token-{index % 2}"},
                )
            )

    assert [response.status_code for response in responses[:60]] == [202] * 60
    assert responses[60].status_code == 429


@pytest.mark.parametrize("days", (7, 30, 90))
async def test_admin_activity_accepts_documented_numeric_day_queries(activity_db, days):
    target = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
    )
    activity_db.add(target)
    await activity_db.commit()

    async with _client(db=activity_db, user=_user(UserRole.ADMIN)) as client:
        response = await client.get(
            f"/api/admin/users/{target.id}/activity?days={days}&kind=browsing"
        )
    assert response.status_code == 200, response.text


async def test_non_admin_cannot_read_user_activity():
    async with _client(user=_user(UserRole.BUYER)) as client:
        response = await client.get(
            f"/api/admin/users/{uuid4()}/activity?kind=browsing"
        )
    assert response.status_code == 403


async def test_ingestion_is_per_user_idempotent_and_timeline_never_crosses_users(activity_db):
    shared_event_id = uuid4()
    first_user = uuid4()
    second_user = uuid4()
    now = datetime(2026, 9, 25, 12, tzinfo=UTC)

    assert await ingest_browsing_events(
        activity_db,
        user_id=first_user,
        payload=_payload(shared_event_id, page="marketplace"),
        received_at=now,
    ) == 1
    assert await ingest_browsing_events(
        activity_db,
        user_id=first_user,
        payload=_payload(shared_event_id, page="marketplace"),
        received_at=now,
    ) == 0
    assert await ingest_browsing_events(
        activity_db,
        user_id=second_user,
        payload=_payload(shared_event_id, page="trades"),
        received_at=now + timedelta(seconds=1),
    ) == 1

    first = await get_user_activity_page(
        activity_db,
        user_id=first_user,
        days=7,
        kind="browsing",
        limit=50,
        offset=0,
        now=now + timedelta(days=1),
    )
    second = await get_user_activity_page(
        activity_db,
        user_id=second_user,
        days=7,
        kind="browsing",
        limit=50,
        offset=0,
        now=now + timedelta(days=1),
    )
    assert first.items[0].details["page"] == "marketplace"
    assert second.items[0].details["page"] == "trades"
    stored = await activity_db.scalar(
        select(UserBrowsingEvent).where(
            UserBrowsingEvent.user_id == first_user,
            UserBrowsingEvent.event_id == shared_event_id,
        )
    )
    assert stored is not None
    assert stored.consent_version is None


async def test_timeline_pagination_is_stable_and_reports_last_activity(activity_db):
    user_id = uuid4()
    now = datetime(2026, 9, 25, 12, tzinfo=UTC)
    for index, page in enumerate(("home", "map", "marketplace")):
        await ingest_browsing_events(
            activity_db,
            user_id=user_id,
            payload=_payload(uuid4(), page=page),
            received_at=now + timedelta(seconds=index),
        )

    first_page = await get_user_activity_page(
        activity_db,
        user_id=user_id,
        days=7,
        kind="browsing",
        limit=2,
        offset=0,
        now=now + timedelta(days=1),
    )
    second_page = await get_user_activity_page(
        activity_db,
        user_id=user_id,
        days=7,
        kind="browsing",
        limit=2,
        offset=2,
        now=now + timedelta(days=1),
    )

    assert [item.details["page"] for item in first_page.items] == ["marketplace", "map"]
    assert first_page.has_more is True
    assert [item.details["page"] for item in second_page.items] == ["home"]
    assert second_page.has_more is False
    assert first_page.last_activity_at == now + timedelta(seconds=2)


async def test_database_event_quota_counts_unique_events_per_account(activity_db, monkeypatch):
    import app.services.user_activity as activity_service

    monkeypatch.setattr(activity_service, "MAX_BROWSING_EVENTS_PER_MINUTE", 2)
    user_id = uuid4()
    now = datetime(2026, 9, 25, 12, tzinfo=UTC)
    first_id = uuid4()
    payload = BrowsingEventsIn.model_validate(
        {
            "events": [
                {"id": str(first_id), "action": "page_view", "page": "home"},
                {"id": str(first_id), "action": "page_view", "page": "home"},
                {"id": str(uuid4()), "action": "page_view", "page": "map"},
            ],
        }
    )
    assert await ingest_browsing_events(
        activity_db,
        user_id=user_id,
        payload=payload,
        received_at=now,
    ) == 2
    # An idempotent replay is still accepted at the quota boundary.
    assert await ingest_browsing_events(
        activity_db,
        user_id=user_id,
        payload=_payload(first_id, page="home"),
        received_at=now,
    ) == 0
    with pytest.raises(HTTPException) as exc_info:
        await ingest_browsing_events(
            activity_db,
            user_id=user_id,
            payload=_payload(uuid4(), page="trades"),
            received_at=now,
        )
    assert getattr(exc_info.value, "status_code", None) == 429


async def test_business_and_login_filters_use_safe_legacy_facts_without_duplicates(activity_db):
    user_id = uuid4()
    now = datetime(2026, 9, 25, 12, tzinfo=UTC)
    target_id = uuid4()
    preference_id = uuid4()
    watchlist_id = uuid4()
    activity_db.add_all(
        [
            UserLoginDay(
                id=uuid4(),
                activity_date=now.date(),
                user_id=user_id,
                role=UserRole.BUYER,
                login_count=3,
                first_login_at=now - timedelta(hours=1),
                last_login_at=now,
            ),
            AuditLog(
                id=uuid4(),
                user_id=user_id,
                action="order.created",
                resource_type="order",
                resource_id=str(uuid4()),
                changes={
                    "market_product": "BIO_METHANOL",
                    "quantity_mt": "500",
                    "reason": "must never be exposed",
                },
                ip_address="203.0.113.8",
                request_id="must-not-leak",
                timestamp=now - timedelta(minutes=1),
            ),
            UserPreference(
                id=preference_id,
                user_id=user_id,
                namespace="market_watch",
                value={"products": ["BIO_ETHANOL"], "portIds": ["Santos"]},
                updated_at=now - timedelta(minutes=2),
            ),
            Watchlist(
                id=watchlist_id,
                user_id=user_id,
                name="Legacy Radar",
                created_at=now - timedelta(minutes=4),
            ),
            WatchlistTarget(
                id=target_id,
                watchlist_id=watchlist_id,
                target_type=WatchlistTargetType.SLICE,
                market_product_code="BIO_ETHANOL",
                delivery_point_id=uuid4(),
                availability_window_code="SPOT",
                snapshot_delivery_point_name="Legacy Port",
                created_at=now - timedelta(minutes=3),
            ),
        ]
    )
    await activity_db.commit()

    business = await get_user_activity_page(
        activity_db,
        user_id=user_id,
        days=7,
        kind="business",
        limit=50,
        offset=0,
        now=now + timedelta(minutes=1),
    )
    assert [item.action for item in business.items] == [
        "order.created",
        "preferences.market_watch_saved",
        "watchlist.saved_target",
    ]
    order_details = business.items[0].details
    assert order_details["market_product"] == "BIO_METHANOL"
    assert order_details["quantity_mt"] == "500"
    assert "reason" not in order_details
    assert "ip_address" not in order_details
    assert business.items[1].details["port_names"] == ["Santos"]
    assert business.items[2].details["delivery_point_name"] == "Legacy Port"

    login = await get_user_activity_page(
        activity_db,
        user_id=user_id,
        days=7,
        kind="login",
        limit=50,
        offset=0,
        now=now + timedelta(minutes=1),
    )
    assert len(login.items) == 1
    assert login.items[0].action == "login_day"
    assert login.items[0].details == {"login_count": 3}

    activity_db.add(
        AuditLog(
            id=uuid4(),
            user_id=user_id,
            action="watchlist.saved_target",
            resource_type="watchlist_target",
            resource_id=str(target_id),
            changes={"market_product": "BIO_ETHANOL"},
            timestamp=now,
        )
    )
    await activity_db.commit()
    deduplicated = await get_user_activity_page(
        activity_db,
        user_id=user_id,
        days=7,
        kind="business",
        limit=50,
        offset=0,
        now=now + timedelta(minutes=1),
    )
    assert sum(item.action == "watchlist.saved_target" for item in deduplicated.items) == 1


def test_schema_accepts_policy_covered_events_and_legacy_marker_but_rejects_invalid_fields():
    from pydantic import ValidationError

    without_consent = BrowsingEventsIn.model_validate(
        {"events": [{"id": str(uuid4()), "action": "page_view", "page": "home"}]}
    )
    assert without_consent.consent_version is None
    with_legacy_marker = BrowsingEventsIn.model_validate(
        {
            "consent_version": 2,
            "events": [{"id": str(uuid4()), "action": "page_view", "page": "home"}],
        }
    )
    assert with_legacy_marker.consent_version == 2

    for payload in (
        {"consent_version": 1, "events": [{"id": str(uuid4()), "action": "page_view", "page": "home"}]},
        {"events": [{"id": str(uuid4()), "action": "page_view", "page": "free-form"}]},
        {"events": [{"id": str(uuid4()), "action": "page_view", "page": "home", "query": "secret"}]},
    ):
        with pytest.raises(ValidationError):
            BrowsingEventsIn.model_validate(payload)

    too_many = {
        "events": [
            {"id": str(uuid4()), "action": "page_view", "page": "home"}
            for _ in range(51)
        ],
    }
    with pytest.raises(ValidationError):
        BrowsingEventsIn.model_validate(too_many)

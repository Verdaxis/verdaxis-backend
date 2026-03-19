"""Unit tests for the Surveillance API endpoints — STORY S6-005.

Uses a minimal FastAPI app (surveillance router only) with dependency overrides
to avoid pulling in sqladmin and other production-only dependencies.
"""
import uuid
import pytest
from datetime import datetime, UTC

from fastapi import FastAPI
from fastapi.testclient import TestClient
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base, get_db
from app.models.user import User, UserRole, UserStatus, Organization, OrgType
from app.models.surveillance import SurveillanceEvent, SurveillanceType, SurveillanceSeverity, SurveillanceStatus
from app.routers.auth_simple import get_current_user
from app.routers.surveillance import router as surveillance_router

_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
    "notifications",
    "surveillance_events",
]

# ---------------------------------------------------------------------------
# Shared engine / session factory (module scope for speed)
# ---------------------------------------------------------------------------

_engine = create_async_engine("sqlite+aiosqlite://", echo=False, future=True)
_session_factory = async_sessionmaker(
    bind=_engine,
    class_=AsyncSession,
    expire_on_commit=False,
    autoflush=False,
)


@pytest.fixture(scope="module", autouse=True)
async def _create_tables():
    tables = [Base.metadata.tables[t] for t in _REQUIRED_TABLES if t in Base.metadata.tables]
    async with _engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with _engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


# ---------------------------------------------------------------------------
# Minimal test app (avoids importing app.main which requires sqladmin)
# ---------------------------------------------------------------------------

def _build_app(current_user: User) -> FastAPI:
    """Build a minimal FastAPI instance with surveillance router and overrides."""
    test_app = FastAPI()
    test_app.include_router(surveillance_router)

    async def _override_db():
        async with _session_factory() as session:
            yield session

    async def _override_user():
        return current_user

    test_app.dependency_overrides[get_db] = _override_db
    test_app.dependency_overrides[get_current_user] = _override_user
    return test_app


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_user(role: UserRole) -> User:
    org = Organization(id=uuid.uuid4(), name="TestOrg", type=OrgType.SHIPPING_LINE)
    user = User(
        id=uuid.uuid4(),
        email=f"{role.value.lower()}-{uuid.uuid4()}@test.com",
        password_hash="hashed",
        role=role,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=org.id,
    )
    user._org = org  # carry org reference for session insertion
    return user


def _make_event(
    event_type: SurveillanceType = SurveillanceType.SPOOFING,
    severity: SurveillanceSeverity = SurveillanceSeverity.MEDIUM,
    status: SurveillanceStatus = SurveillanceStatus.OPEN,
) -> SurveillanceEvent:
    return SurveillanceEvent(
        id=uuid.uuid4(),
        type=event_type,
        severity=severity,
        status=status,
        participants=[str(uuid.uuid4())],
        related_trades=[str(uuid.uuid4())],
        description="Test event",
        auto_detected=True,
        created_at=datetime.now(UTC),
        updated_at=datetime.now(UTC),
    )


async def _seed(user: User, *events: SurveillanceEvent) -> None:
    """Insert user, their org, and events into the DB."""
    async with _session_factory() as session:
        session.add(user._org)
        await session.flush()
        session.add(user)
        await session.flush()
        for event in events:
            session.add(event)
        await session.commit()


# ---------------------------------------------------------------------------
# Tests — list events
# ---------------------------------------------------------------------------

class TestListEvents:
    @pytest.mark.asyncio
    async def test_list_events_requires_compliance_role(self):
        """A BUYER gets HTTP 403 when accessing /surveillance/events."""
        buyer = _make_user(UserRole.BUYER)
        await _seed(buyer, _make_event())

        client = TestClient(_build_app(buyer))
        resp = client.get("/surveillance/events")
        assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_list_events_requires_compliance_role_supplier(self):
        """A SUPPLIER also gets HTTP 403."""
        supplier = _make_user(UserRole.SUPPLIER)
        await _seed(supplier, _make_event())

        client = TestClient(_build_app(supplier))
        resp = client.get("/surveillance/events")
        assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_list_events_as_admin(self):
        """An ADMIN can list surveillance events."""
        admin = _make_user(UserRole.ADMIN)
        await _seed(admin, _make_event())

        client = TestClient(_build_app(admin))
        resp = client.get("/surveillance/events")
        assert resp.status_code == 200
        data = resp.json()
        assert isinstance(data, list)
        assert len(data) >= 1

    @pytest.mark.asyncio
    async def test_list_events_as_compliance_officer(self):
        """A COMPLIANCE_OFFICER can list surveillance events."""
        officer = _make_user(UserRole.COMPLIANCE_OFFICER)
        await _seed(officer, _make_event())

        client = TestClient(_build_app(officer))
        resp = client.get("/surveillance/events")
        assert resp.status_code == 200
        data = resp.json()
        assert isinstance(data, list)

    @pytest.mark.asyncio
    async def test_filter_by_type(self):
        """?type=SPOOFING only returns SPOOFING events in the result set."""
        officer = _make_user(UserRole.COMPLIANCE_OFFICER)
        spoof_event = _make_event(event_type=SurveillanceType.SPOOFING)
        wash_event = _make_event(event_type=SurveillanceType.WASH_TRADING)
        await _seed(officer, spoof_event, wash_event)

        client = TestClient(_build_app(officer))
        resp = client.get("/surveillance/events?type=SPOOFING")
        assert resp.status_code == 200
        for item in resp.json():
            assert item["type"] == "SPOOFING"

    @pytest.mark.asyncio
    async def test_filter_by_severity(self):
        """?severity=HIGH only returns HIGH events."""
        officer = _make_user(UserRole.COMPLIANCE_OFFICER)
        high_event = _make_event(severity=SurveillanceSeverity.HIGH)
        low_event = _make_event(severity=SurveillanceSeverity.LOW)
        await _seed(officer, high_event, low_event)

        client = TestClient(_build_app(officer))
        resp = client.get("/surveillance/events?severity=HIGH")
        assert resp.status_code == 200
        for item in resp.json():
            assert item["severity"] == "HIGH"

    @pytest.mark.asyncio
    async def test_filter_by_status(self):
        """?status=OPEN only returns OPEN events."""
        admin = _make_user(UserRole.ADMIN)
        open_event = _make_event(status=SurveillanceStatus.OPEN)
        closed_event = _make_event(status=SurveillanceStatus.CLOSED)
        await _seed(admin, open_event, closed_event)

        client = TestClient(_build_app(admin))
        resp = client.get("/surveillance/events?status=OPEN")
        assert resp.status_code == 200
        for item in resp.json():
            assert item["status"] == "OPEN"


# ---------------------------------------------------------------------------
# Tests — update event
# ---------------------------------------------------------------------------

class TestUpdateEvent:
    @pytest.mark.asyncio
    async def test_update_event_status(self):
        """PATCH /surveillance/events/{id} updates status to REVIEWING."""
        officer = _make_user(UserRole.COMPLIANCE_OFFICER)
        event = _make_event()
        await _seed(officer, event)

        client = TestClient(_build_app(officer))
        resp = client.patch(
            f"/surveillance/events/{event.id}",
            json={"status": "REVIEWING"},
        )
        assert resp.status_code == 200
        data = resp.json()
        assert data["status"] == "REVIEWING"

    @pytest.mark.asyncio
    async def test_update_event_notes(self):
        """PATCH /surveillance/events/{id} sets notes."""
        admin = _make_user(UserRole.ADMIN)
        event = _make_event()
        await _seed(admin, event)

        client = TestClient(_build_app(admin))
        resp = client.patch(
            f"/surveillance/events/{event.id}",
            json={"notes": "Escalated to senior compliance officer."},
        )
        assert resp.status_code == 200
        data = resp.json()
        assert data["notes"] == "Escalated to senior compliance officer."

    @pytest.mark.asyncio
    async def test_update_event_requires_auth(self):
        """PATCH by a BUYER returns 403."""
        buyer = _make_user(UserRole.BUYER)
        event = _make_event()
        await _seed(buyer, event)

        client = TestClient(_build_app(buyer))
        resp = client.patch(
            f"/surveillance/events/{event.id}",
            json={"status": "CLOSED"},
        )
        assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_update_nonexistent_event(self):
        """PATCH unknown UUID returns 404."""
        admin = _make_user(UserRole.ADMIN)
        event = _make_event()
        await _seed(admin, event)

        client = TestClient(_build_app(admin))
        resp = client.patch(
            f"/surveillance/events/{uuid.uuid4()}",
            json={"status": "CLOSED"},
        )
        assert resp.status_code == 404

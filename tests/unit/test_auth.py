"""
Unit tests for security/authentication utilities.
Updated for PyJWT + direct bcrypt (2026-03-01).
"""
import bcrypt as _bcrypt
import pytest

from app.core.security import (
    verify_password, get_password_hash,
    create_access_token, create_refresh_token, decode_token,
)


@pytest.fixture(autouse=True)
def _fast_bcrypt(monkeypatch):
    """The wrapper logic, not KDF strength, is under test; default 12-round
    bcrypt made these six tests the slowest in the suite (~5s combined)."""
    real_gensalt = _bcrypt.gensalt
    monkeypatch.setattr(_bcrypt, "gensalt", lambda *a, **k: real_gensalt(rounds=4))


class TestPasswordHashing:
    def test_get_password_hash_returns_hash(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert hashed != password
        assert len(hashed) > 20

    def test_get_password_hash_different_each_time(self):
        password = "mysecurepassword"
        hash1 = get_password_hash(password)
        hash2 = get_password_hash(password)
        assert hash1 != hash2

    def test_verify_password_correct(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert verify_password(password, hashed) is True

    def test_verify_password_incorrect(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert verify_password("wrongpassword", hashed) is False

    def test_verify_password_empty_string(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert verify_password("", hashed) is False

    def test_hash_empty_password(self):
        hashed = get_password_hash("")
        assert hashed is not None
        assert len(hashed) > 0


class TestTokenCreation:
    def test_create_access_token_returns_jwt(self):
        token = create_access_token(subject="user123")
        assert isinstance(token, str)
        assert token.count('.') == 2

    def test_create_access_token_with_custom_expiry(self):
        from datetime import timedelta
        token = create_access_token(subject="user123", expires_delta=timedelta(minutes=30))
        assert isinstance(token, str)
        assert token.count('.') == 2

    def test_token_contains_user_data(self):
        user_id = "test-user-id-123"
        token = create_access_token(
            subject=user_id,
            additional_claims={"role": "SUPPLIER"},
        )
        payload = decode_token(token)
        assert payload["sub"] == user_id
        assert payload["role"] == "SUPPLIER"
        assert payload["type"] == "access"
        assert "exp" in payload
        assert "iat" in payload

    def test_refresh_token_has_correct_type(self):
        token = create_refresh_token(subject="user123")
        payload = decode_token(token)
        assert payload["type"] == "refresh"
        assert "exp" in payload
        assert "iat" in payload

    def test_access_token_rejected_as_refresh(self):
        """Access tokens should have type='access', not 'refresh'."""
        token = create_access_token(subject="user123")
        payload = decode_token(token)
        assert payload["type"] == "access"

    def test_expired_token_raises(self):
        from datetime import timedelta
        import jwt as pyjwt
        token = create_access_token(subject="user123", expires_delta=timedelta(seconds=-1))
        with pytest.raises(pyjwt.ExpiredSignatureError):
            decode_token(token)


# ---------------------------------------------------------------------------
# Login-day facts (Product Analytics plan §2.4)
# ---------------------------------------------------------------------------

from datetime import UTC, datetime, timedelta
from uuid import uuid4

from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from slowapi.errors import RateLimitExceeded
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base, get_db
from app.models.product_analytics import UserLoginDay
from app.models.refresh_session import RefreshSession
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.rate_limit import limiter
from app.routers.auth_simple import router as auth_router
from app.services.product_analytics import record_login_day


@pytest.fixture(autouse=True)
def _disable_rate_limits(monkeypatch):
    monkeypatch.setattr(limiter, "enabled", False)


@pytest.fixture
async def login_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = [Organization.__table__, User.__table__, UserLoginDay.__table__, RefreshSession.__table__]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


def _login_app(session) -> FastAPI:
    app = FastAPI()
    app.state.limiter = limiter
    app.add_exception_handler(RateLimitExceeded, lambda _r, _e: None)
    app.include_router(auth_router, prefix="/api")

    async def override_db():
        yield session

    app.dependency_overrides[get_db] = override_db
    return app


async def _seed_login_user(session, *, role=UserRole.BUYER, password="correct-horse-9"):
    org = Organization(id=uuid4(), name=f"Org {uuid4()}", type=OrgType.FUEL_BUYER)
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash=get_password_hash(password),
        role=role,
        status=UserStatus.APPROVED,
        organization_id=org.id,
        email_verified=True,
    )
    session.add_all([org, user])
    await session.commit()
    return user, password


async def _login(app, email: str, password: str):
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        return await client.post(
            "/api/auth/login", data={"username": email, "password": password}
        )


@pytest.mark.asyncio
async def test_successful_login_upserts_one_login_day_row(login_db):
    user, password = await _seed_login_user(login_db)
    app = _login_app(login_db)

    response = await _login(app, user.email, password)
    assert response.status_code == 200, response.text

    rows = (await login_db.execute(select(UserLoginDay))).scalars().all()
    assert len(rows) == 1
    row = rows[0]
    assert row.user_id == user.id
    assert row.organization_id == user.organization_id
    assert row.role == UserRole.BUYER
    assert row.login_count == 1
    assert row.first_login_at == row.last_login_at
    assert row.activity_date == datetime.now(UTC).date()
    # Never store IP, user agent, token, or credential data.
    stored_columns = {column.name for column in UserLoginDay.__table__.columns}
    assert stored_columns.isdisjoint({"ip", "ip_address", "user_agent", "token", "password"})


@pytest.mark.asyncio
async def test_second_same_day_login_increments_the_same_row(login_db):
    user, password = await _seed_login_user(login_db)
    app = _login_app(login_db)

    first = await _login(app, user.email, password)
    second = await _login(app, user.email, password)
    assert first.status_code == 200 and second.status_code == 200

    rows = (await login_db.execute(select(UserLoginDay))).scalars().all()
    assert len(rows) == 1
    assert rows[0].login_count == 2
    assert rows[0].last_login_at >= rows[0].first_login_at


@pytest.mark.asyncio
async def test_failed_login_writes_no_login_day_row(login_db):
    user, _password = await _seed_login_user(login_db)
    app = _login_app(login_db)

    response = await _login(app, user.email, "wrong-password")
    assert response.status_code == 401

    rows = (await login_db.execute(select(UserLoginDay))).scalars().all()
    assert rows == []


@pytest.mark.asyncio
async def test_admin_logins_are_stored_with_their_role_snapshot(login_db):
    user, password = await _seed_login_user(login_db, role=UserRole.ADMIN)
    app = _login_app(login_db)

    response = await _login(app, user.email, password)
    assert response.status_code == 200

    rows = (await login_db.execute(select(UserLoginDay))).scalars().all()
    assert len(rows) == 1
    # Stored, but excluded from member metrics by the analytics service.
    assert rows[0].role == UserRole.ADMIN


@pytest.mark.asyncio
async def test_next_utc_day_creates_a_new_row_and_keeps_earliest_first_login(login_db):
    user, _password = await _seed_login_user(login_db)

    day_one_morning = datetime(2026, 7, 10, 8, tzinfo=UTC)
    day_one_evening = datetime(2026, 7, 10, 21, tzinfo=UTC)
    day_two = datetime(2026, 7, 11, 7, tzinfo=UTC)
    await record_login_day(login_db, user, at=day_one_evening)
    await record_login_day(login_db, user, at=day_one_morning)  # out-of-order arrival
    await record_login_day(login_db, user, at=day_two)
    await login_db.commit()

    rows = (
        (await login_db.execute(select(UserLoginDay).order_by(UserLoginDay.activity_date)))
        .scalars()
        .all()
    )
    assert [row.activity_date.isoformat() for row in rows] == ["2026-07-10", "2026-07-11"]
    day_one = rows[0]
    assert day_one.login_count == 2
    assert day_one.first_login_at.replace(tzinfo=UTC) == day_one_morning
    assert day_one.last_login_at.replace(tzinfo=UTC) == day_one_evening
    assert rows[1].login_count == 1

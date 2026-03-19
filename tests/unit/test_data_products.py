"""
Unit tests for the tiered data products endpoint — S7-005.

Tests:
  - test_public_gets_daily_vwap_only
  - test_authenticated_gets_hourly_vwap
  - test_paid_tier_gets_realtime
"""
import uuid
import pytest

from fastapi import FastAPI
from fastapi.testclient import TestClient
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base, get_db
from app.routers.data_products import router as data_products_router
from app.core.security import create_access_token

# ---------------------------------------------------------------------------
# Tables needed by this test module
# ---------------------------------------------------------------------------

_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
]

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
# Helpers
# ---------------------------------------------------------------------------


def _build_app() -> FastAPI:
    from slowapi import Limiter
    from slowapi.util import get_remote_address
    from slowapi.errors import RateLimitExceeded
    from fastapi.responses import JSONResponse

    test_app = FastAPI()
    test_app.state.limiter = Limiter(key_func=get_remote_address)

    @test_app.exception_handler(RateLimitExceeded)
    async def _rl(request, exc):
        return JSONResponse(status_code=429, content={"detail": "rate limited"})

    test_app.include_router(data_products_router)

    async def _override_db():
        async with _session_factory() as session:
            yield session

    test_app.dependency_overrides[get_db] = _override_db
    return test_app


def _user_token(user_id: uuid.UUID) -> str:
    """Mint a regular user session token (tier: free)."""
    return create_access_token(
        subject=str(user_id),
        additional_claims={"role": "BUYER"},
    )


def _oauth_token(tier: str = "free") -> str:
    """Mint an OAuth2 client token with the given rate_limit_tier."""
    return create_access_token(
        subject=str(uuid.uuid4()),
        additional_claims={
            "token_kind": "oauth2_client",
            "scopes": ["read:market"],
            "rate_limit_tier": tier,
        },
    )


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestTieredReferencePrices:
    def test_public_gets_daily_vwap_only(self):
        """Unauthenticated caller gets 200 with resolution='daily'."""
        app = _build_app()
        tc = TestClient(app)
        resp = tc.get("/data/reference-prices")
        assert resp.status_code == 200
        data = resp.json()
        assert data["tier"] == "public"
        assert data["resolution"] == "daily"
        assert "prices" in data
        assert "note" in data  # upsell hint

    def test_authenticated_gets_hourly_vwap(self):
        """User session token (free tier) gets resolution='hourly'."""
        app = _build_app()
        tc = TestClient(app)
        token = _user_token(uuid.uuid4())
        resp = tc.get("/data/reference-prices", headers={"Authorization": f"Bearer {token}"})
        assert resp.status_code == 200
        data = resp.json()
        assert data["tier"] == "free"
        assert data["resolution"] == "hourly"
        assert "prices" in data

    def test_paid_tier_gets_realtime(self):
        """OAuth2 client with paid tier gets resolution='realtime' + metadata."""
        app = _build_app()
        tc = TestClient(app)
        token = _oauth_token(tier="paid")
        resp = tc.get("/data/reference-prices", headers={"Authorization": f"Bearer {token}"})
        assert resp.status_code == 200
        data = resp.json()
        assert data["tier"] == "paid"
        assert data["resolution"] == "realtime"
        assert "metadata" in data
        assert "prices" in data

    def test_free_oauth_token_gets_hourly(self):
        """OAuth2 client with free tier gets resolution='hourly' (same as user session)."""
        app = _build_app()
        tc = TestClient(app)
        token = _oauth_token(tier="free")
        resp = tc.get("/data/reference-prices", headers={"Authorization": f"Bearer {token}"})
        assert resp.status_code == 200
        data = resp.json()
        assert data["tier"] == "free"
        assert data["resolution"] == "hourly"

    def test_invalid_token_falls_back_to_public(self):
        """Malformed/expired token is treated as unauthenticated (public tier)."""
        app = _build_app()
        tc = TestClient(app)
        resp = tc.get(
            "/data/reference-prices",
            headers={"Authorization": "Bearer this.is.not.valid"},
        )
        assert resp.status_code == 200
        data = resp.json()
        assert data["tier"] == "public"

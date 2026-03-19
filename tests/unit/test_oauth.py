"""
Unit tests for the OAuth2 client self-service API — S7-002/S7-003/S7-004.

Uses a minimal FastAPI app (oauth router only) with dependency overrides.
All DB operations use an in-memory SQLite engine.
"""
import uuid
import pytest

from fastapi import FastAPI
from fastapi.testclient import TestClient
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base, get_db
from app.models.user import User, UserRole, UserStatus, Organization, OrgType
from app.routers.auth_simple import get_current_user
from app.routers.oauth import router as oauth_router
from app.core.security import create_access_token

# ---------------------------------------------------------------------------
# Tables needed by this test module
# ---------------------------------------------------------------------------

_REQUIRED_TABLES = [
    "organizations",
    "users",
    "oauth_clients",
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
# Helpers
# ---------------------------------------------------------------------------

def _make_user(suffix: str = "") -> User:
    org = Organization(id=uuid.uuid4(), name="TestOrg", type=OrgType.SHIPPING_LINE)
    user = User(
        id=uuid.uuid4(),
        email=f"user{suffix}-{uuid.uuid4()}@test.com",
        password_hash="hashed",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=org.id,
    )
    user._org = org
    return user


async def _seed_user(user: User) -> None:
    async with _session_factory() as session:
        session.add(user._org)
        await session.flush()
        session.add(user)
        await session.commit()


def _build_app(current_user: User) -> FastAPI:
    test_app = FastAPI()
    test_app.include_router(oauth_router)

    async def _override_db():
        async with _session_factory() as session:
            yield session

    async def _override_user():
        return current_user

    test_app.dependency_overrides[get_db] = _override_db
    test_app.dependency_overrides[get_current_user] = _override_user
    return test_app


def _user_token(user: User) -> str:
    return create_access_token(
        subject=str(user.id),
        additional_claims={"role": user.role.value},
    )


# ---------------------------------------------------------------------------
# S7-002: Client CRUD
# ---------------------------------------------------------------------------


class TestCreateClient:
    @pytest.mark.asyncio
    async def test_create_client_returns_secret_once(self):
        """POST /oauth/clients returns client_secret in the response body (shown once)."""
        user = _make_user("create")
        await _seed_user(user)

        app = _build_app(user)
        client = TestClient(app)
        resp = client.post("/oauth/clients", json={"name": "MyApp", "scopes": ["read:market"]})

        assert resp.status_code == 201
        data = resp.json()
        assert "client_id" in data
        assert "client_secret" in data
        assert len(data["client_secret"]) > 20
        assert data["name"] == "MyApp"
        assert "read:market" in data["scopes"]

    @pytest.mark.asyncio
    async def test_create_client_rejects_invalid_scope(self):
        """Invalid scopes are rejected with HTTP 422."""
        user = _make_user("badscope")
        await _seed_user(user)

        app = _build_app(user)
        client = TestClient(app)
        resp = client.post("/oauth/clients", json={"name": "Bad", "scopes": ["admin:everything"]})
        assert resp.status_code == 422


class TestListClients:
    @pytest.mark.asyncio
    async def test_list_clients_shows_own_only(self):
        """GET /oauth/clients returns only the current user's clients."""
        user_a = _make_user("lista")
        user_b = _make_user("listb")
        await _seed_user(user_a)
        await _seed_user(user_b)

        # Create a client owned by user_b
        app_b = _build_app(user_b)
        tc_b = TestClient(app_b)
        tc_b.post("/oauth/clients", json={"name": "B-App", "scopes": ["read:market"]})

        # user_a list should be empty
        app_a = _build_app(user_a)
        tc_a = TestClient(app_a)
        resp = tc_a.get("/oauth/clients")
        assert resp.status_code == 200
        assert resp.json() == []


class TestDeleteClient:
    @pytest.mark.asyncio
    async def test_delete_client_removes_it(self):
        """DELETE /oauth/clients/{client_id} removes the client."""
        user = _make_user("del")
        await _seed_user(user)

        app = _build_app(user)
        tc = TestClient(app)
        create_resp = tc.post("/oauth/clients", json={"name": "ToDelete", "scopes": ["read:market"]})
        assert create_resp.status_code == 201
        client_id = create_resp.json()["client_id"]

        del_resp = tc.delete(f"/oauth/clients/{client_id}")
        assert del_resp.status_code == 204

        list_resp = tc.get("/oauth/clients")
        client_ids = [c["client_id"] for c in list_resp.json()]
        assert client_id not in client_ids

    @pytest.mark.asyncio
    async def test_delete_client_other_owner_returns_404(self):
        """A user cannot delete another user's client.

        Returns 404 (not 403) to prevent existence leakage — the ownership
        check is folded into the DB query so other users' clients are
        indistinguishable from non-existent ones.
        """
        owner = _make_user("owner")
        attacker = _make_user("attacker")
        await _seed_user(owner)
        await _seed_user(attacker)

        app_owner = _build_app(owner)
        tc_owner = TestClient(app_owner)
        create_resp = tc_owner.post("/oauth/clients", json={"name": "OwnerApp", "scopes": ["read:market"]})
        client_id = create_resp.json()["client_id"]

        app_attacker = _build_app(attacker)
        tc_attacker = TestClient(app_attacker)
        resp = tc_attacker.delete(f"/oauth/clients/{client_id}")
        assert resp.status_code == 404


# ---------------------------------------------------------------------------
# S7-003: Token endpoint
# ---------------------------------------------------------------------------


class TestTokenEndpoint:
    @pytest.mark.asyncio
    async def test_token_valid_credentials_returns_jwt(self):
        """Valid client_credentials returns an access_token JWT."""
        user = _make_user("tok")
        await _seed_user(user)

        app = _build_app(user)
        tc = TestClient(app)
        create_resp = tc.post(
            "/oauth/clients",
            json={"name": "TokenApp", "scopes": ["read:market", "read:orders"]},
        )
        assert create_resp.status_code == 201
        payload = create_resp.json()

        token_resp = tc.post(
            "/oauth/token",
            json={
                "grant_type": "client_credentials",
                "client_id": payload["client_id"],
                "client_secret": payload["client_secret"],
            },
        )
        assert token_resp.status_code == 200
        tok = token_resp.json()
        assert "access_token" in tok
        assert tok["token_type"] == "bearer"
        assert tok["expires_in"] > 0

    @pytest.mark.asyncio
    async def test_token_invalid_credentials_returns_401(self):
        """Wrong client_secret returns HTTP 401."""
        user = _make_user("badcreds")
        await _seed_user(user)

        app = _build_app(user)
        tc = TestClient(app)
        create_resp = tc.post("/oauth/clients", json={"name": "BadCreds", "scopes": ["read:market"]})
        client_id = create_resp.json()["client_id"]

        resp = tc.post(
            "/oauth/token",
            json={
                "grant_type": "client_credentials",
                "client_id": client_id,
                "client_secret": "wrong-secret",
            },
        )
        assert resp.status_code == 401

    @pytest.mark.asyncio
    async def test_token_jwt_contains_scopes(self):
        """The issued JWT embeds the client's scopes in the 'scopes' claim."""
        import jwt as pyjwt
        from app.core.security import SECRET_KEY, ALGORITHM

        user = _make_user("scopes")
        await _seed_user(user)

        app = _build_app(user)
        tc = TestClient(app)
        scopes = ["read:market", "read:compliance"]
        create_resp = tc.post("/oauth/clients", json={"name": "ScopesApp", "scopes": scopes})
        payload = create_resp.json()

        token_resp = tc.post(
            "/oauth/token",
            json={
                "grant_type": "client_credentials",
                "client_id": payload["client_id"],
                "client_secret": payload["client_secret"],
            },
        )
        access_token = token_resp.json()["access_token"]
        decoded = pyjwt.decode(access_token, SECRET_KEY, algorithms=[ALGORITHM])
        assert set(decoded["scopes"]) == set(scopes)
        assert decoded["token_kind"] == "oauth2_client"

    @pytest.mark.asyncio
    async def test_token_unknown_client_id_returns_401(self):
        """Unknown client_id returns HTTP 401 (not 404 — avoids client enumeration)."""
        user = _make_user("unknown")
        await _seed_user(user)

        app = _build_app(user)
        tc = TestClient(app)
        resp = tc.post(
            "/oauth/token",
            json={
                "grant_type": "client_credentials",
                "client_id": str(uuid.uuid4()),
                "client_secret": "doesnt-matter",
            },
        )
        assert resp.status_code == 401


# ---------------------------------------------------------------------------
# S7-004: Scope enforcement
# ---------------------------------------------------------------------------


class TestScopeEnforcement:
    """
    Tests for require_scope() dependency.
    We build a tiny app with a single route protected by require_scope.
    """

    def _build_scope_app(self, required_scope: str) -> FastAPI:
        from fastapi import FastAPI as _FA
        from app.routers.oauth import require_scope

        _app = _FA()

        @_app.get("/protected", dependencies=[require_scope(required_scope)])
        async def _protected():
            return {"ok": True}

        return _app

    def _oauth_token_with_scopes(self, scopes: list[str]) -> str:
        """Mint a fake OAuth2 client token with the given scopes."""
        return create_access_token(
            subject=str(uuid.uuid4()),
            additional_claims={
                "token_kind": "oauth2_client",
                "scopes": scopes,
                "rate_limit_tier": "free",
            },
        )

    def _user_login_token(self) -> str:
        """Mint a regular user login token (no token_kind claim)."""
        return create_access_token(
            subject=str(uuid.uuid4()),
            additional_claims={"role": "BUYER"},
        )

    def test_scope_enforcement_allows_matching_scope(self):
        """OAuth2 token WITH required scope passes through."""
        app = self._build_scope_app("read:market")
        tc = TestClient(app)
        token = self._oauth_token_with_scopes(["read:market", "read:orders"])
        resp = tc.get("/protected", headers={"Authorization": f"Bearer {token}"})
        assert resp.status_code == 200

    def test_scope_enforcement_rejects_missing_scope(self):
        """OAuth2 token WITHOUT required scope gets HTTP 403."""
        app = self._build_scope_app("write:orders")
        tc = TestClient(app)
        token = self._oauth_token_with_scopes(["read:market"])
        resp = tc.get("/protected", headers={"Authorization": f"Bearer {token}"})
        assert resp.status_code == 403

    def test_user_login_tokens_bypass_scope_check(self):
        """User session tokens (no token_kind) bypass scope enforcement."""
        app = self._build_scope_app("write:orders")
        tc = TestClient(app)
        token = self._user_login_token()
        resp = tc.get("/protected", headers={"Authorization": f"Bearer {token}"})
        assert resp.status_code == 200

    def test_no_token_returns_401(self):
        """Unauthenticated request returns HTTP 401."""
        app = self._build_scope_app("read:market")
        tc = TestClient(app)
        resp = tc.get("/protected")
        assert resp.status_code == 401

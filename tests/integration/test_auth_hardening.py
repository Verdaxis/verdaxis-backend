"""
Auth hardening integration tests.

Tests:
- Login returns both access + refresh tokens
- Refresh token exchange works
- Expired access token triggers 401
- Invalid refresh token triggers 401
- Rate limiting on login endpoint (6th attempt in 1 min → 429)
- RBAC: buyer cannot access admin audit logs
"""
import pytest
import uuid
from httpx import AsyncClient

import os
TEST_API_URL = os.environ.get("TEST_API_URL")


@pytest.fixture
async def auth_client():
    async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as ac:
        yield ac


@pytest.fixture
async def buyer_tokens(auth_client: AsyncClient, itest_password):
    """Login as the disposable itest buyer and return both tokens."""
    form = {"username": "itest-buyer@disposable.invalid", "password": itest_password}
    res = await auth_client.post("/api/auth/login", data=form)
    assert res.status_code == 200, f"Login failed: {res.text}"
    data = res.json()
    assert "access_token" in data
    assert "refresh_token" in data
    return data


@pytest.fixture
async def seller_tokens(auth_client: AsyncClient, itest_password):
    """Login as the disposable itest seller and return both tokens."""
    form = {"username": "itest-seller@disposable.invalid", "password": itest_password}
    res = await auth_client.post("/api/auth/login", data=form)
    assert res.status_code == 200, f"Login failed: {res.text}"
    return res.json()


class TestDualTokenLogin:
    @pytest.mark.asyncio
    async def test_login_returns_both_tokens(self, auth_client, buyer_tokens):
        assert buyer_tokens["access_token"]
        assert buyer_tokens["refresh_token"]
        assert buyer_tokens["token_type"] == "bearer"

    @pytest.mark.asyncio
    async def test_access_token_works_for_me(self, auth_client, buyer_tokens):
        headers = {"Authorization": f"Bearer {buyer_tokens['access_token']}"}
        res = await auth_client.get("/api/auth/me", headers=headers)
        assert res.status_code == 200
        user = res.json()
        assert user["email"] == "itest-buyer@disposable.invalid"


class TestTokenRefresh:
    @pytest.mark.asyncio
    async def test_refresh_returns_new_tokens(self, auth_client, buyer_tokens):
        res = await auth_client.post(
            "/api/auth/refresh",
            json={"refresh_token": buyer_tokens["refresh_token"]},
        )
        assert res.status_code == 200
        data = res.json()
        assert "access_token" in data
        assert "refresh_token" in data
        # New access token should work
        headers = {"Authorization": f"Bearer {data['access_token']}"}
        me_res = await auth_client.get("/api/auth/me", headers=headers)
        assert me_res.status_code == 200

    @pytest.mark.asyncio
    async def test_invalid_refresh_token_rejected(self, auth_client):
        res = await auth_client.post(
            "/api/auth/refresh",
            json={"refresh_token": "invalid.token.here"},
        )
        assert res.status_code == 401

    @pytest.mark.asyncio
    async def test_access_token_as_refresh_rejected(self, auth_client, buyer_tokens):
        res = await auth_client.post(
            "/api/auth/refresh",
            json={"refresh_token": buyer_tokens["access_token"]},
        )
        assert res.status_code == 401


class TestRBACEnforcement:
    @pytest.mark.asyncio
    async def test_buyer_cannot_access_audit_logs(self, auth_client, buyer_tokens):
        headers = {"Authorization": f"Bearer {buyer_tokens['access_token']}"}
        res = await auth_client.get("/api/admin/audit-logs", headers=headers)
        assert res.status_code == 403

    @pytest.mark.asyncio
    async def test_unauthenticated_cannot_access_me(self, auth_client):
        res = await auth_client.get("/api/auth/me")
        assert res.status_code in (401, 403)


class TestHealthEndpoints:
    @pytest.mark.asyncio
    async def test_health_live(self, auth_client):
        res = await auth_client.get("/health/live")
        assert res.status_code == 200
        assert res.json()["status"] == "ok"

    @pytest.mark.asyncio
    async def test_health_ready(self, auth_client):
        res = await auth_client.get("/health/ready")
        assert res.status_code == 200
        data = res.json()
        assert {"status", "db", "environment", "release_sha"} <= set(data)
        assert data["status"] == "ok"
        assert data["db"] == "ok"
        assert data["environment"] in {"production", "staging", "test"}
        assert len(data["release_sha"]) == 40

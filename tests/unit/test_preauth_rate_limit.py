"""Tests for the pre-auth rate-limit middleware (Sprint 3 items 5 & 9)."""
import pytest

from app.middleware import preauth_rate_limit as prl


@pytest.fixture(autouse=True)
def _clean_buckets():
    prl._buckets.clear()
    yield
    prl._buckets.clear()


class TestCheck:
    def test_allows_up_to_limit(self):
        for _ in range(5):
            assert prl._check("/x", 5, 60, "1.2.3.4", now=100.0)
        assert not prl._check("/x", 5, 60, "1.2.3.4", now=100.0)

    def test_window_resets(self):
        for _ in range(5):
            assert prl._check("/x", 5, 60, "1.2.3.4", now=100.0)
        assert not prl._check("/x", 5, 60, "1.2.3.4", now=159.0)
        assert prl._check("/x", 5, 60, "1.2.3.4", now=161.0)

    def test_ips_have_independent_buckets(self):
        for _ in range(5):
            assert prl._check("/x", 5, 60, "1.2.3.4", now=100.0)
        assert not prl._check("/x", 5, 60, "1.2.3.4", now=100.0)
        assert prl._check("/x", 5, 60, "5.6.7.8", now=100.0)

    def test_prefixes_have_independent_buckets(self):
        for _ in range(5):
            assert prl._check("/a", 5, 60, "1.2.3.4", now=100.0)
        assert prl._check("/b", 5, 60, "1.2.3.4", now=100.0)

    def test_bucket_cap_clears_instead_of_growing(self):
        prl._buckets.update(
            {("/x", f"ip{i}"): (0.0, 1) for i in range(prl._MAX_BUCKETS)}
        )
        assert prl._check("/x", 5, 60, "new-ip", now=100.0)
        assert len(prl._buckets) == 1


class TestClientIp:
    def _request(self, headers: dict[str, str], client_host: str = "127.0.0.1"):
        class _Client:
            host = client_host

        class _Request:
            def __init__(self):
                self.headers = headers
                self.client = _Client()

        return _Request()

    def test_uses_last_forwarded_hop(self):
        # Attacker-supplied XFF prefix must not shadow the Caddy-appended hop.
        req = self._request({"X-Forwarded-For": "6.6.6.6, 203.0.113.9"})
        assert prl.client_ip(req) == "203.0.113.9"

    def test_direct_non_loopback_peer_cannot_spoof_forwarded_address(self):
        req = self._request(
            {"X-Forwarded-For": "198.51.100.99"},
            client_host="203.0.113.8",
        )
        assert prl.client_ip(req) == "203.0.113.8"

    def test_falls_back_to_peer_address(self):
        req = self._request({}, client_host="10.0.0.5")
        assert prl.client_ip(req) == "10.0.0.5"


class TestMiddlewareIntegration:
    @pytest.mark.parametrize("path", [
        "/api/auth/admin/invitations",
        "/api/auth/admin/review-queue",
        "/api/auth/approve/user-id",
        "/api/auth/reject/user-id",
        "/api/auth/organization/org-id/approve",
        "/api/auth/organization-joins",
        "/api/auth/organization-joins/request-id/reject",
    ])
    @pytest.mark.asyncio
    async def test_limits_auth_admin_paths_before_dependencies(self, path):
        from fastapi import Depends, FastAPI, HTTPException
        from httpx import ASGITransport, AsyncClient

        app = FastAPI()
        app.middleware("http")(prl.preauth_rate_limit_middleware)

        def reject_invalid_token():
            raise HTTPException(status_code=401, detail="Invalid token")

        @app.get(path, dependencies=[Depends(reject_invalid_token)])
        async def protected_endpoint():
            pytest.fail("Unauthenticated request reached the endpoint")

        prefix, limit, window = next(
            rule for rule in prl.PREAUTH_LIMITS if path.startswith(rule[0])
        )
        transport = ASGITransport(app=app, client=("198.51.100.7", 1234))
        async with AsyncClient(transport=transport, base_url="http://t") as client:
            assert (await client.get(path)).status_code == 401
            started, _ = prl._buckets[(prefix, "198.51.100.7")]
            prl._buckets[(prefix, "198.51.100.7")] = (started, limit)
            response = await client.get(path)
            assert response.status_code == 429
            assert response.headers["Retry-After"] == str(window)

    @pytest.mark.asyncio
    async def test_limits_unauthenticated_admin_requests(self):
        from fastapi import FastAPI
        from httpx import ASGITransport, AsyncClient

        app = FastAPI()
        app.middleware("http")(prl.preauth_rate_limit_middleware)

        @app.get("/api/admin/analytics/overview")
        async def overview():
            return {"ok": True}

        limit = dict(
            (p, (l, w)) for p, l, w in prl.PREAUTH_LIMITS
        )["/api/admin/"][0]

        transport = ASGITransport(app=app)
        async with AsyncClient(transport=transport, base_url="http://t") as client:
            headers = {"X-Forwarded-For": "198.51.100.7"}
            for _ in range(limit):
                r = await client.get("/api/admin/analytics/overview", headers=headers)
                assert r.status_code == 200
            r = await client.get("/api/admin/analytics/overview", headers=headers)
            assert r.status_code == 429
            assert r.headers["Retry-After"]
            # Unrelated paths are untouched even while the bucket is exhausted
            r = await client.get("/api/admin/analytics/overview", headers={"X-Forwarded-For": "198.51.100.8"})
            assert r.status_code == 200

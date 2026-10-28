"""Only the attested test wrapper can reset the real limiter between tests."""
from unittest.mock import Mock

import httpx
import pytest

from tests.disposable_server import DisposableIdentityApp, IDENTITY_PATH


@pytest.mark.asyncio
async def test_rate_limit_reset_requires_disposable_secret_and_post(monkeypatch):
    from app.rate_limit import limiter

    reset = Mock()
    monkeypatch.setattr(limiter, "reset", reset)

    async def application(scope, receive, send):
        raise AssertionError("Test control must not reach the product application")

    token = "a" * 32
    transport = httpx.ASGITransport(app=DisposableIdentityApp(application, token))
    async with httpx.AsyncClient(transport=transport, base_url="http://127.0.0.1") as client:
        path = IDENTITY_PATH + "/reset-limits"
        assert (await client.post(path)).status_code == 403
        assert (await client.post(path, headers={"X-Disposable-Token": "wrong"})).status_code == 403
        assert (await client.get(path, headers={"X-Disposable-Token": token})).status_code == 403
        reset.assert_not_called()
        response = await client.post(path, headers={"X-Disposable-Token": token})
        assert response.status_code == 204
        assert response.headers["cache-control"] == "no-store"
        reset.assert_called_once_with()

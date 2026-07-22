"""Frontend idempotency contract: CORS must admit the Idempotency-Key header.

The frontend (hardening/frontend-v2 @ 28a72c2c) sends Idempotency-Key on
POST /api/orderbook and POST /api/trades/ from app.verdaxis.exchange. The
backend uses header reflection (allow_headers=["*"] reflects the preflight's
Access-Control-Request-Headers), which admits the header today; this test
pins the contract so switching to an explicit header allowlist cannot
silently break credentialed order placement.
"""
from __future__ import annotations

import httpx
import pytest

from app.config import settings
from app.main import app

pytestmark = pytest.mark.asyncio


@pytest.mark.parametrize("path", ["/api/orderbook", "/api/trades/"])
async def test_preflight_admits_idempotency_key_with_exact_origin(path):
    origin = settings.BACKEND_CORS_ORIGINS[0]
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
        response = await client.options(
            path,
            headers={
                "Origin": origin,
                "Access-Control-Request-Method": "POST",
                "Access-Control-Request-Headers": "authorization,content-type,idempotency-key",
            },
        )
    assert response.status_code == 200
    allowed = {
        value.strip().lower()
        for value in response.headers.get("access-control-allow-headers", "").split(",")
    }
    assert "idempotency-key" in allowed
    assert response.headers.get("access-control-allow-origin") == origin
    assert response.headers.get("access-control-allow-credentials") == "true"

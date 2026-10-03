"""Frontend idempotency contract: CORS must admit the Idempotency-Key header.

The frontend (hardening/frontend-v2 @ 28a72c2c) sends Idempotency-Key on
POST /api/orderbook and POST /api/trades/ from app.verdaxis.exchange. The
backend uses header reflection (allow_headers=["*"] reflects the preflight's
Access-Control-Request-Headers), which admits the header today; this test
pins the contract so switching to an explicit header allowlist cannot
silently break credentialed order placement.
"""
from __future__ import annotations

import time
from uuid import UUID

import httpx
import pytest

from app.config import settings
from app.main import app
from app.middleware import preauth_rate_limit as prl
from app.routing import MAX_MARKET_JSON_BODY_BYTES, MarketJSONBodyLimitRoute
from app.services.request_party import (
    MARKET_SUPPORT_CONTEXT_HEADER,
    MARKET_SUPPORT_CONTEXT_INVALID_HEADER,
)

pytestmark = pytest.mark.asyncio

_CLIENT_IP = "198.51.100.77"
_REQUEST_ID = "cors-retry-test"
_LOGIN_PREFIX, _LOGIN_LIMIT, _LOGIN_WINDOW = prl.PREAUTH_LIMITS[0]

_SMALL_MARKET_PAYLOADS = (
    (
        "/api/orderbook",
        {
            "side": "BID",
            "product_id": "00000000-0000-0000-0000-000000000001",
            "delivery_point_id": "00000000-0000-0000-0000-000000000002",
            "quantity_mt": 1,
            "price_per_mt_usd": 1,
        },
    ),
    (
        "/api/trades/",
        {
            "order_id": "00000000-0000-0000-0000-000000000001",
            "quantity_mt": 1,
        },
    ),
)


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


async def test_exhausted_preauth_bucket_has_browser_readable_retry_headers():
    origin = settings.BACKEND_CORS_ORIGINS[0]
    key = (_LOGIN_PREFIX, _CLIENT_IP)
    previous = prl._buckets.get(key)
    prl._buckets[key] = (time.monotonic(), _LOGIN_LIMIT)
    try:
        transport = httpx.ASGITransport(app=app)
        async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
            allowed_response = await client.post(
                _LOGIN_PREFIX,
                headers={
                    "Origin": origin,
                    "X-Forwarded-For": _CLIENT_IP,
                    "X-Request-ID": _REQUEST_ID,
                },
            )
            disallowed_response = await client.post(
                _LOGIN_PREFIX,
                headers={
                    "Origin": "https://untrusted.example",
                    "X-Forwarded-For": _CLIENT_IP,
                    "X-Request-ID": _REQUEST_ID,
                },
            )
    finally:
        if previous is None:
            prl._buckets.pop(key, None)
        else:
            prl._buckets[key] = previous

    assert allowed_response.status_code == 429
    assert allowed_response.headers["Retry-After"] == str(_LOGIN_WINDOW)
    assert allowed_response.headers["X-Request-ID"] == _REQUEST_ID
    assert allowed_response.headers["access-control-allow-origin"] == origin
    assert allowed_response.headers["access-control-allow-credentials"] == "true"
    exposed = {
        value.strip().lower()
        for value in allowed_response.headers["access-control-expose-headers"].split(",")
    }
    assert exposed == {
        "retry-after",
        "x-request-id",
        MARKET_SUPPORT_CONTEXT_INVALID_HEADER.lower(),
    }
    assert disallowed_response.status_code == 429
    assert "access-control-allow-origin" not in disallowed_response.headers


async def test_sensitive_preflight_does_not_consume_preauth_bucket():
    origin = settings.BACKEND_CORS_ORIGINS[0]
    key = (_LOGIN_PREFIX, _CLIENT_IP)
    previous = prl._buckets.get(key)
    prl._buckets.pop(key, None)
    try:
        transport = httpx.ASGITransport(app=app)
        async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
            response = await client.options(
                _LOGIN_PREFIX,
                headers={
                    "Origin": origin,
                    "X-Forwarded-For": _CLIENT_IP,
                    "Access-Control-Request-Method": "POST",
                    "Access-Control-Request-Headers": (
                        "authorization,content-type,idempotency-key,"
                        + MARKET_SUPPORT_CONTEXT_HEADER
                    ),
                },
            )
        assert response.status_code == 200
        assert key not in prl._buckets
        allowed = {
            value.strip().lower()
            for value in response.headers["access-control-allow-headers"].split(",")
        }
        assert {
            "authorization",
            "content-type",
            "idempotency-key",
            MARKET_SUPPORT_CONTEXT_HEADER.lower(),
        } <= allowed
    finally:
        if previous is None:
            prl._buckets.pop(key, None)
        else:
            prl._buckets[key] = previous


async def test_support_scope_rejection_keeps_credentialed_cors():
    origin = settings.BACKEND_CORS_ORIGINS[0]
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
        response = await client.post(
            f"/api/trades/{UUID(int=1)}/pay",
            headers={
                "Origin": origin,
                MARKET_SUPPORT_CONTEXT_HEADER: str(UUID(int=2)),
            },
        )

    assert response.status_code == 403
    assert response.headers["access-control-allow-origin"] == origin
    assert response.headers["access-control-allow-credentials"] == "true"


async def test_missing_authentication_keeps_credentialed_cors():
    origin = settings.BACKEND_CORS_ORIGINS[0]
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
        response = await client.get("/api/auth/me", headers={"Origin": origin})

    assert response.status_code == 401
    assert response.headers["access-control-allow-origin"] == origin
    assert response.headers["access-control-allow-credentials"] == "true"


@pytest.mark.parametrize(("path", "_payload"), _SMALL_MARKET_PAYLOADS)
async def test_market_json_limit_rejects_declared_and_chunked_oversize_before_auth(
    path,
    _payload,
):
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
        declared = await client.post(
            path,
            content=b"{}",
            headers={"Content-Length": str(MAX_MARKET_JSON_BODY_BYTES + 1)},
        )

        async def chunks():
            yield b'{"metadata":"'
            yield b"x" * MAX_MARKET_JSON_BODY_BYTES
            yield b'"}'

        chunked = await client.post(
            path,
            content=chunks(),
            headers={"Content-Type": "application/json"},
        )

    expected_detail = MarketJSONBodyLimitRoute.body_too_large_detail
    assert declared.status_code == 413
    assert declared.json() == {"detail": expected_detail}
    assert chunked.status_code == 413
    assert chunked.json() == {"detail": expected_detail}


@pytest.mark.parametrize(("path", "payload"), _SMALL_MARKET_PAYLOADS)
async def test_small_market_json_reaches_authentication(path, payload):
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://t") as client:
        response = await client.post(path, json=payload)

    assert response.status_code == 401

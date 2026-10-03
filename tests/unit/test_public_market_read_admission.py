from __future__ import annotations

import asyncio
import json

import httpx
import pytest
from starlette.responses import JSONResponse

from app.config import settings
from app.database import get_db
from app.main import app as main_app
from app.middleware.public_market_read_admission import (
    PublicMarketReadAdmissionMiddleware,
)


pytestmark = pytest.mark.asyncio


async def _send_ok(scope, receive, send) -> None:
    await JSONResponse({"ok": True})(scope, receive, send)


async def _request(asgi_app, path: str, *, method: str = "GET"):
    transport = httpx.ASGITransport(app=asgi_app)
    async with httpx.AsyncClient(
        transport=transport,
        base_url="http://test",
    ) as client:
        response = await client.request(method, path)

    return response.status_code, response.headers, response.content


class _BlockingApp:
    def __init__(self) -> None:
        self.started: asyncio.Queue[str] = asyncio.Queue()
        self.release = asyncio.Event()

    async def __call__(self, scope, receive, send) -> None:
        await self.started.put(scope["path"])
        await self.release.wait()
        await _send_ok(scope, receive, send)


async def test_two_selected_reads_hold_capacity_and_third_fails_immediately():
    downstream = _BlockingApp()
    middleware = PublicMarketReadAdmissionMiddleware(
        downstream,
        capacity=2,
        api_prefix="/api",
    )
    admitted = [
        asyncio.create_task(_request(middleware, "/api/availability")),
        asyncio.create_task(_request(middleware, "/api/orderbook")),
    ]
    try:
        await asyncio.wait_for(downstream.started.get(), timeout=1)
        await asyncio.wait_for(downstream.started.get(), timeout=1)

        status_code, headers, body = await asyncio.wait_for(
            _request(middleware, "/api/prices/reference"),
            timeout=0.25,
        )
    finally:
        downstream.release.set()
        admitted_responses = await asyncio.gather(*admitted)

    assert [response[0] for response in admitted_responses] == [200, 200]
    assert status_code == 503
    assert headers["retry-after"] == "1"
    assert json.loads(body) == {
        "detail": "Public market data is temporarily busy; retry shortly."
    }


@pytest.mark.parametrize(
    "path",
    [
        "/api/listings",
        "/api/ports/SGSIN",
    ],
)
async def test_zero_capacity_rejects_selected_reads(path):
    middleware = PublicMarketReadAdmissionMiddleware(
        _send_ok,
        capacity=0,
        api_prefix="/api",
    )

    status_code, headers, _body = await _request(middleware, path)

    assert status_code == 503
    assert headers["retry-after"] == "1"


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("POST", "/api/orderbook"),
        ("GET", "/api/orderbook/my"),
        ("GET", "/api/listings/my"),
        ("GET", "/api/trades/summary"),
        ("GET", "/api/stream/prices"),
        ("GET", "/api/auth/me"),
        ("GET", "/health/ready"),
        ("GET", "/api/news"),
        ("GET", "/api/referrals"),
        ("GET", "/api/ports/SGSIN/extra"),
    ],
)
async def test_unselected_http_requests_bypass_admission(method, path):
    middleware = PublicMarketReadAdmissionMiddleware(
        _send_ok,
        capacity=0,
        api_prefix="/api",
    )

    status_code, _headers, _body = await _request(
        middleware,
        path,
        method=method,
    )

    assert status_code == 200


async def test_non_http_scope_bypasses_admission():
    seen_scopes = []

    async def downstream(scope, receive, send):
        seen_scopes.append(scope)

    middleware = PublicMarketReadAdmissionMiddleware(
        downstream,
        capacity=0,
        api_prefix="/api",
    )
    scope = {"type": "websocket", "path": "/api/orderbook"}

    async def receive():
        return {"type": "websocket.disconnect", "code": 1000}

    async def send(message):
        raise AssertionError(f"unexpected message: {message}")

    await middleware(scope, receive, send)

    assert seen_scopes == [scope]


class _CancelledThenSuccessfulApp:
    def __init__(self) -> None:
        self.calls = 0
        self.started = asyncio.Event()
        self.block = asyncio.Event()

    async def __call__(self, scope, receive, send) -> None:
        self.calls += 1
        if self.calls == 1:
            self.started.set()
            await self.block.wait()
        await _send_ok(scope, receive, send)


async def test_cancelled_request_releases_admission():
    downstream = _CancelledThenSuccessfulApp()
    middleware = PublicMarketReadAdmissionMiddleware(
        downstream,
        capacity=1,
        api_prefix="/api",
    )
    cancelled = asyncio.create_task(_request(middleware, "/api/demand"))
    await asyncio.wait_for(downstream.started.wait(), timeout=1)

    cancelled.cancel()
    with pytest.raises(asyncio.CancelledError):
        await cancelled

    status_code, _headers, _body = await _request(middleware, "/api/demand")

    assert status_code == 200


class _FailedThenSuccessfulApp:
    def __init__(self) -> None:
        self.calls = 0

    async def __call__(self, scope, receive, send) -> None:
        self.calls += 1
        if self.calls == 1:
            raise RuntimeError("downstream failed")
        await _send_ok(scope, receive, send)


async def test_downstream_exception_releases_admission():
    downstream = _FailedThenSuccessfulApp()
    middleware = PublicMarketReadAdmissionMiddleware(
        downstream,
        capacity=1,
        api_prefix="/api",
    )

    with pytest.raises(RuntimeError, match="downstream failed"):
        await _request(middleware, "/api/benchmarks")

    status_code, _headers, _body = await _request(
        middleware,
        "/api/benchmarks",
    )

    assert status_code == 200


class _EmptyResult:
    def all(self):
        return []


class _EmptyAvailabilitySession:
    async def execute(self, statement):
        return _EmptyResult()


async def test_registered_cors_wraps_saturated_admission_response():
    started: asyncio.Queue[None] = asyncio.Queue()
    release = asyncio.Event()

    async def blocked_db():
        await started.put(None)
        await release.wait()
        yield _EmptyAvailabilitySession()

    missing = object()
    previous_override = main_app.dependency_overrides.get(get_db, missing)
    main_app.dependency_overrides[get_db] = blocked_db
    admitted = []
    try:
        transport = httpx.ASGITransport(app=main_app)
        async with httpx.AsyncClient(
            transport=transport,
            base_url="http://test",
        ) as client:
            admitted = [
                asyncio.create_task(client.get("/api/availability")),
                asyncio.create_task(client.get("/api/availability")),
            ]
            await asyncio.wait_for(started.get(), timeout=1)
            await asyncio.wait_for(started.get(), timeout=1)

            origin = settings.BACKEND_CORS_ORIGINS[0]
            response = await asyncio.wait_for(
                client.get("/api/availability", headers={"Origin": origin}),
                timeout=0.25,
            )
            release.set()
            admitted_responses = await asyncio.gather(*admitted)
    finally:
        release.set()
        if admitted:
            await asyncio.gather(*admitted, return_exceptions=True)
        if previous_override is missing:
            main_app.dependency_overrides.pop(get_db, None)
        else:
            main_app.dependency_overrides[get_db] = previous_override

    assert [item.status_code for item in admitted_responses] == [200, 200]
    assert response.status_code == 503
    assert response.headers["Retry-After"] == "1"
    assert response.headers["access-control-allow-origin"] == origin
    assert response.headers["access-control-allow-credentials"] == "true"
    exposed = {
        value.strip().lower()
        for value in response.headers["access-control-expose-headers"].split(",")
    }
    assert "retry-after" in exposed

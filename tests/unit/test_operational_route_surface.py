"""Security contract for the deliberately small public operational surface."""

from unittest.mock import AsyncMock

import pytest
from fastapi.responses import JSONResponse

from app import main


def _route_methods() -> set[tuple[str, str]]:
    return {
        (method, route.path)
        for route in main.app.routes
        for method in getattr(route, "methods", set())
    }


def test_logs_host_metrics_manual_news_refresh_and_role_switch_are_not_routes():
    routes = _route_methods()

    assert ("GET", "/api/dashboard/logs") not in routes
    assert ("GET", "/api/dashboard/health") not in routes
    assert ("POST", "/api/news/refresh") not in routes
    assert ("POST", "/api/compliance/verify") not in routes
    assert ("GET", "/api/compliance/ledger") not in routes
    assert not any("switch-role" in path for _, path in routes)
    assert ("GET", "/api/news") in routes
    assert ("GET", "/api/compliance/fuels") in routes


def test_sanitized_readiness_route_remains():
    assert ("GET", "/health/ready") in _route_methods()


@pytest.mark.asyncio
async def test_readiness_failure_exposes_no_backend_exception(monkeypatch):
    class BrokenConnection:
        async def __aenter__(self):
            raise RuntimeError("postgresql://user:secret@private.example/database")

        async def __aexit__(self, *args):
            return False

    engine = AsyncMock()
    engine.connect = lambda: BrokenConnection()
    monkeypatch.setattr("app.database.engine", engine)

    response = await main.health_ready()

    assert isinstance(response, JSONResponse)
    assert response.status_code == 503
    assert response.body == b'{"status":"error","db":"unavailable"}'

import asyncio
import json

import pytest

from app import main


class _FailingConnection:
    async def __aenter__(self):
        raise RuntimeError("postgresql://role:credential@database.example/verdaxis")

    async def __aexit__(self, *args):
        return None


class _SlowConnection:
    async def __aenter__(self):
        await asyncio.sleep(0.05)
        return self

    async def __aexit__(self, *args):
        return None


class _HealthyConnection:
    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return None

    async def execute(self, statement):
        return None


class _Engine:
    def __init__(self, connection):
        self.connection = connection

    def connect(self):
        return self.connection


@pytest.mark.asyncio
@pytest.mark.parametrize("connection", [_FailingConnection(), _SlowConnection()])
async def test_readiness_failure_and_timeout_are_bounded_and_sanitized(
    monkeypatch, connection
):
    calls = []
    monkeypatch.setattr("app.database.engine", _Engine(connection))
    monkeypatch.setattr(main.settings, "HEALTH_READINESS_TIMEOUT_SECONDS", 0.01)
    monkeypatch.setattr(main.settings, "ENVIRONMENT", "test")
    monkeypatch.setattr(main.settings, "RELEASE_SHA", "test")
    monkeypatch.setattr(
        main,
        "logger",
        type("Logger", (), {"exception": lambda self, *args, **kwargs: calls.append(1)})(),
    )

    response = await main.health_ready()
    payload = json.loads(response.body)

    assert response.status_code == 503
    assert payload == {
        "status": "error",
        "db": "unavailable",
        "environment": "test",
        "release_sha": "test",
    }
    assert b"credential" not in response.body
    assert calls


@pytest.mark.asyncio
async def test_legacy_health_alias_uses_readiness(monkeypatch):
    sentinel = object()

    async def ready():
        return sentinel

    monkeypatch.setattr(main, "health_ready", ready)
    assert await main.health_check() is sentinel


@pytest.mark.asyncio
async def test_readiness_exposes_sanitized_deployment_provenance(monkeypatch):
    monkeypatch.setattr("app.database.engine", _Engine(_HealthyConnection()))
    monkeypatch.setattr(main.settings, "ENVIRONMENT", "test")
    monkeypatch.setattr(main.settings, "RELEASE_SHA", "test")

    assert await main.health_ready() == {
        "status": "ok",
        "db": "connected",
        "environment": "test",
        "release_sha": "test",
    }


@pytest.mark.asyncio
async def test_liveness_remains_process_only():
    assert await main.health_live() == {"status": "ok"}

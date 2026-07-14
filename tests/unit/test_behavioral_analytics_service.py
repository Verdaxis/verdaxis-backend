import asyncio
import logging
from uuid import uuid4

import httpx
import pytest

from app.config import Settings
from app.services.behavioral_analytics import (
    ADMIN_FEATURE_EVENT_NAMES,
    AnalyticsDiagnostic,
    AnalyticsEvent,
    FRONTEND_EVENT_NAMES,
    SERVER_EVENT_NAMES,
    UmamiAnalyticsService,
)


def _settings(**overrides) -> Settings:
    values = {
        "DATABASE_URL": "sqlite+aiosqlite:///:memory:",
        "DATABASE_PASSWORD": "test-password",
        "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
        "ANALYTICS_ENABLED": True,
        "UMAMI_BASE_URL": "https://analytics.example.com/",
        "UMAMI_WEBSITE_ID": "website-id",
        "UMAMI_API_USERNAME": "api-user",
        "UMAMI_API_PASSWORD": "super-secret-password",
        "ANALYTICS_REQUEST_TIMEOUT_SECONDS": 1.0,
    }
    values.update(overrides)
    return Settings(**values)


def test_disabled_or_empty_analytics_configuration_does_not_block_settings_startup():
    disabled = _settings(
        ANALYTICS_ENABLED=False,
        UMAMI_BASE_URL=None,
        UMAMI_WEBSITE_ID=None,
        UMAMI_API_USERNAME=None,
        UMAMI_API_PASSWORD=None,
    )
    incomplete = _settings(
        UMAMI_BASE_URL=None,
        UMAMI_WEBSITE_ID=None,
        UMAMI_API_USERNAME=None,
        UMAMI_API_PASSWORD=None,
    )

    assert disabled.ANALYTICS_ENABLED is False
    assert incomplete.ANALYTICS_ENABLED is True


@pytest.mark.parametrize("timeout", [0, -1, 10.1])
def test_analytics_timeout_is_bounded(timeout):
    with pytest.raises(ValueError):
        _settings(ANALYTICS_REQUEST_TIMEOUT_SECONDS=timeout)


@pytest.mark.asyncio
async def test_authentication_token_is_cached_between_aggregate_requests():
    auth_calls = 0

    def handler(request: httpx.Request) -> httpx.Response:
        nonlocal auth_calls
        if request.url.path == "/api/auth/login":
            auth_calls += 1
            return httpx.Response(200, json={"token": "token-value"})
        if request.url.path.endswith("/stats"):
            return httpx.Response(200, json={"visitors": 2, "visits": 3, "pageviews": 4, "totaltime": 12})
        if request.url.path.endswith("/pageviews"):
            return httpx.Response(200, json={"pageviews": [], "sessions": []})
        if request.url.path.endswith("/events/series"):
            return httpx.Response(200, json=[])
        return httpx.Response(200, json=[])

    service = UmamiAnalyticsService(
        _settings(),
        transport=httpx.MockTransport(handler),
        success_cache_seconds=0,
    )

    await service.get_aggregate(7)
    await service.get_aggregate(30)

    assert auth_calls == 1


@pytest.mark.asyncio
async def test_timeout_returns_coarse_unavailable_result_without_secrets(caplog):
    async def handler(_request: httpx.Request) -> httpx.Response:
        raise httpx.ReadTimeout("body contains super-secret-password")

    service = UmamiAnalyticsService(
        _settings(),
        transport=httpx.MockTransport(handler),
    )

    with caplog.at_level(logging.WARNING):
        result = await service.get_aggregate(7)

    assert result.status == "unavailable"
    assert result.diagnostic == AnalyticsDiagnostic.TIMEOUT
    assert "super-secret-password" not in caplog.text
    assert "body contains" not in caplog.text


@pytest.mark.asyncio
async def test_malformed_or_oversized_upstream_response_is_unavailable():
    def malformed(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/api/auth/login":
            return httpx.Response(200, content=b"not-json")
        raise AssertionError("data endpoint should not be reached")

    malformed_service = UmamiAnalyticsService(
        _settings(), transport=httpx.MockTransport(malformed)
    )
    malformed_result = await malformed_service.get_aggregate(7)
    assert malformed_result.diagnostic == AnalyticsDiagnostic.MALFORMED_RESPONSE

    def oversized(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=b"x" * 1_000_001)

    oversized_service = UmamiAnalyticsService(
        _settings(), transport=httpx.MockTransport(oversized)
    )
    oversized_result = await oversized_service.get_aggregate(7)
    assert oversized_result.diagnostic == AnalyticsDiagnostic.MALFORMED_RESPONSE


@pytest.mark.asyncio
async def test_success_and_failure_aggregates_use_separate_bounded_caches():
    calls = 0

    def handler(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        return httpx.Response(503)

    service = UmamiAnalyticsService(
        _settings(),
        transport=httpx.MockTransport(handler),
        success_cache_seconds=300,
        failure_cache_seconds=30,
    )
    first = await service.get_aggregate(7)
    second = await service.get_aggregate(7)

    assert first.status == second.status == "unavailable"
    assert calls == 1
    assert service.success_cache_seconds <= 300
    assert service.failure_cache_seconds <= 30


@pytest.mark.asyncio
async def test_server_event_payload_is_allowlisted_and_contains_no_commercial_or_pii_fields():
    captured: dict = {}
    captured_headers: dict = {}

    def handler(request: httpx.Request) -> httpx.Response:
        captured.update(__import__("json").loads(request.content))
        captured_headers.update(request.headers)
        return httpx.Response(200, json={"sessionId": "ignored"})

    service = UmamiAnalyticsService(
        _settings(FRONTEND_URL="https://staging.verdaxis.exchange"),
        transport=httpx.MockTransport(handler),
    )
    delivered = await service.send_event(
        AnalyticsEvent(
            name="order_created",
            user_id=uuid4(),
            role="BUYER",
            side="BID",
            canonical_product="BIO_METHANOL",
            delivery_point="Singapore",
            availability_window="SPOT",
            user_agent="Mozilla/5.0 Test Browser",
        )
    )

    payload = captured["payload"]
    assert delivered is True
    assert payload["name"] == "order_created"
    assert payload["hostname"] == "staging.verdaxis.exchange"
    assert captured_headers["user-agent"] == "Mozilla/5.0 Test Browser"
    assert set(payload["data"]) == {
        "user_id",
        "role",
        "side",
        "canonical_product",
        "delivery_point",
        "availability_window",
    }
    forbidden = {
        "email", "name", "organization_name", "order_id", "trade_id",
        "counterparty", "price", "quantity", "total_value",
    }
    assert forbidden.isdisjoint(payload["data"])


def test_server_and_frontend_reporting_event_taxonomies_are_separate_and_documented():
    assert SERVER_EVENT_NAMES == {
        "registration_completed", "organization_created", "order_created", "trade_created",
    }
    assert {
        "landing_cta_clicked", "energy_calculator_started", "energy_calculator_completed",
        "public_language_changed", "signup_started", "signup_role_selected",
        "signup_submitted", "signup_organization_required", "signup_organization_submitted",
        "login_submitted", "login_succeeded", "login_failed", "platform_navigation",
        "market_slice_selected", "listing_opened", "order_form_opened",
        "order_form_submitted", "trade_confirmation_opened", "tutorial_started",
        "tutorial_step_completed", "tutorial_step_skipped", "tutorial_completed",
        "estimator_opened", "estimator_completed",
    } == FRONTEND_EVENT_NAMES
    assert ADMIN_FEATURE_EVENT_NAMES < FRONTEND_EVENT_NAMES
    assert "platform_navigation" in ADMIN_FEATURE_EVENT_NAMES
    assert "signup_started" not in ADMIN_FEATURE_EVENT_NAMES


@pytest.mark.asyncio
async def test_browser_events_are_retained_in_admin_aggregate_and_unknown_events_are_dropped():
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/api/auth/login":
            return httpx.Response(200, json={"token": "token-value"})
        if request.url.path.endswith("/stats"):
            return httpx.Response(200, json={"visitors": 2, "visits": 2, "pageviews": 4, "totaltime": 10})
        if request.url.path.endswith("/pageviews"):
            return httpx.Response(200, json={"pageviews": [], "sessions": []})
        if request.url.path.endswith("/events/series"):
            return httpx.Response(200, json=[
                {"x": "platform_navigation", "t": "2026-07-14T00:00:00Z", "y": 3},
                {"x": "listing_opened", "t": "2026-07-14T00:00:00Z", "y": 2},
                {"x": "unknown_event", "t": "2026-07-14T00:00:00Z", "y": 99},
            ])
        if request.url.params.get("type") == "event":
            return httpx.Response(200, json=[
                {"x": "platform_navigation", "y": 1},
                {"x": "listing_opened", "y": 1},
                {"x": "unknown_event", "y": 99},
            ])
        return httpx.Response(200, json=[])

    service = UmamiAnalyticsService(_settings(), transport=httpx.MockTransport(handler))
    result = await service.get_aggregate(7)

    assert result.event_totals == {"platform_navigation": 3, "listing_opened": 2}
    assert result.event_series == [
        {"date": "2026-07-14", "event": "listing_opened", "value": 2},
        {"date": "2026-07-14", "event": "platform_navigation", "value": 3},
    ]


@pytest.mark.asyncio
async def test_beep_boop_bot_filter_response_is_treated_as_dropped_delivery():
    def handler(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"beep": "boop"})

    service = UmamiAnalyticsService(_settings(), transport=httpx.MockTransport(handler))
    delivered = await service.send_event(
        AnalyticsEvent(
            name="trade_created",
            user_id=uuid4(),
            role="BUYER",
            user_agent="Mozilla/5.0 Test Browser",
        )
    )

    assert delivered is False


@pytest.mark.asyncio
async def test_best_effort_scheduling_never_raises_to_the_request(monkeypatch):
    service = UmamiAnalyticsService(_settings())

    async def fail(_event):
        raise RuntimeError("upstream unavailable")

    monkeypatch.setattr(service, "send_event", fail)
    task = service.schedule_event(
        AnalyticsEvent(name="registration_completed", user_id=uuid4(), role="BUYER")
    )

    assert task in service.pending_tasks
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    assert task.done()
    assert task.exception() is None
    assert not service.pending_tasks

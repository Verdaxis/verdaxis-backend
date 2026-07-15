"""Bounded, best-effort Umami integration for aggregate and conversion analytics."""

from __future__ import annotations

import asyncio
import json
import logging
import secrets
import time
from collections import OrderedDict
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from enum import Enum
from typing import Any
from urllib.parse import urlparse

import httpx

from app.config import Settings, settings

logger = logging.getLogger(__name__)

_MAX_RESPONSE_BYTES = 1_000_000
_TOKEN_TTL_SECONDS = 300
# Windowed aggregate cache (plan §2.6): bounded LRU with per-key
# single-flight locks. Keys include the normalized window, requested
# breakdowns, website id, and a schema-version constant.
_WINDOW_CACHE_MAX_ENTRIES = 32
_WINDOW_CACHE_SCHEMA_VERSION = 1
_LEGACY_WINDOW_QUANTUM_SECONDS = 300
_MAX_PROPERTY_EVENTS = 12
_MAX_PROPERTY_ROWS = 500
SERVER_EVENT_NAMES = frozenset(
    {"registration_completed", "organization_created", "order_created", "trade_created"}
)
FRONTEND_EVENT_NAMES = frozenset(
    {
        "landing_cta_clicked", "energy_calculator_started", "energy_calculator_completed",
        "public_language_changed", "signup_started", "signup_role_selected",
        "signup_submitted", "signup_organization_required", "signup_organization_submitted",
        "login_submitted", "login_succeeded", "login_failed", "platform_navigation",
        "market_slice_selected", "listing_opened", "order_form_opened", "order_form_submitted",
        "trade_confirmation_opened", "tutorial_started", "tutorial_step_completed",
        "tutorial_step_skipped", "tutorial_completed", "estimator_opened", "estimator_completed",
    }
)
ADMIN_FEATURE_EVENT_NAMES = frozenset(
    {
        "platform_navigation", "market_slice_selected", "listing_opened", "order_form_opened",
        "order_form_submitted", "trade_confirmation_opened", "tutorial_started",
        "tutorial_step_completed", "tutorial_step_skipped", "tutorial_completed",
        "estimator_opened", "estimator_completed",
    }
)
_ADMIN_REPORT_EVENT_NAMES = SERVER_EVENT_NAMES | FRONTEND_EVENT_NAMES
_FALLBACK_BROWSER_USER_AGENT = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
)


class AnalyticsDiagnostic(str, Enum):
    DISABLED = "disabled"
    CONFIGURATION = "configuration"
    AUTHENTICATION = "authentication"
    TIMEOUT = "timeout"
    UPSTREAM = "upstream"
    MALFORMED_RESPONSE = "malformed_response"


@dataclass(frozen=True)
class AnalyticsEvent:
    name: str
    role: str
    side: str | None = None
    canonical_product: str | None = None
    delivery_point: str | None = None
    availability_window: str | None = None
    user_agent: str | None = None
    hostname: str | None = None

    def __post_init__(self) -> None:
        if self.name not in SERVER_EVENT_NAMES:
            raise ValueError("Unsupported analytics event")
        object.__setattr__(self, "user_agent", _bounded_user_agent(self.user_agent))
        object.__setattr__(self, "hostname", _bounded_hostname(self.hostname))

    def data(self) -> dict[str, str]:
        values = {
            "role": self.role,
            "side": self.side,
            "canonical_product": self.canonical_product,
            "delivery_point": self.delivery_point,
            "availability_window": self.availability_window,
        }
        return {key: value for key, value in values.items() if value is not None}


@dataclass(frozen=True)
class BehavioralAggregate:
    status: str
    diagnostic: AnalyticsDiagnostic | None
    observed_at: datetime
    visitors: int = 0
    visits: int = 0
    pageviews: int = 0
    total_time_seconds: int = 0
    event_totals: dict[str, int] = field(default_factory=dict)
    event_series: list[dict[str, Any]] = field(default_factory=list)
    daily_visitors: list[dict[str, Any]] = field(default_factory=list)
    top_entries: list[dict[str, Any]] = field(default_factory=list)
    top_referrers: list[dict[str, Any]] = field(default_factory=list)

    @classmethod
    def unavailable(cls, diagnostic: AnalyticsDiagnostic) -> "BehavioralAggregate":
        return cls(
            status="unavailable",
            diagnostic=diagnostic,
            observed_at=datetime.now(UTC),
        )


@dataclass(frozen=True)
class BehavioralWindowAggregate:
    """Date-bounded behavioral aggregate for one half-open UTC window.

    ``status`` is ``available`` when every requested piece resolved,
    ``partial`` when the core aggregate resolved but one or more per-event
    property breakdowns failed (failed events listed in
    ``property_failures``), and ``unavailable`` when the core fetch failed.
    """

    status: str
    diagnostic: AnalyticsDiagnostic | None
    observed_at: datetime
    start: datetime
    end: datetime
    visitors: int = 0
    visits: int = 0
    pageviews: int = 0
    total_time_seconds: int = 0
    event_totals: dict[str, int] = field(default_factory=dict)
    event_series: list[dict[str, Any]] = field(default_factory=list)
    daily_visitors: list[dict[str, Any]] = field(default_factory=list)
    top_entries: list[dict[str, Any]] = field(default_factory=list)
    top_referrers: list[dict[str, Any]] = field(default_factory=list)
    # Per requested event name: [{"property": str, "value": str, "total": int}]
    event_properties: dict[str, list[dict[str, Any]]] = field(default_factory=dict)
    property_failures: tuple[str, ...] = ()

    @classmethod
    def unavailable(
        cls, diagnostic: AnalyticsDiagnostic, start: datetime, end: datetime
    ) -> "BehavioralWindowAggregate":
        return cls(
            status="unavailable",
            diagnostic=diagnostic,
            observed_at=datetime.now(UTC),
            start=start,
            end=end,
        )


class _AnalyticsError(Exception):
    def __init__(self, diagnostic: AnalyticsDiagnostic):
        self.diagnostic = diagnostic
        super().__init__(diagnostic.value)


class UmamiAnalyticsService:
    def __init__(
        self,
        configuration: Settings,
        *,
        transport: httpx.AsyncBaseTransport | None = None,
        success_cache_seconds: int = 300,
        failure_cache_seconds: int = 30,
    ) -> None:
        self.configuration = configuration
        self.transport = transport
        self.success_cache_seconds = min(max(success_cache_seconds, 0), 300)
        self.failure_cache_seconds = min(max(failure_cache_seconds, 0), 30)
        self._token: str | None = None
        self._token_expires_at = 0.0
        self._token_lock = asyncio.Lock()
        # Bounded LRU of windowed aggregates with per-key single-flight locks.
        self._window_cache: OrderedDict[tuple, tuple[float, BehavioralWindowAggregate]] = (
            OrderedDict()
        )
        self._window_locks: dict[tuple, asyncio.Lock] = {}
        self._pending_tasks: set[asyncio.Task[None]] = set()

    @property
    def pending_tasks(self) -> frozenset[asyncio.Task[None]]:
        return frozenset(self._pending_tasks)

    @property
    def _collector_configured(self) -> bool:
        return bool(
            self.configuration.ANALYTICS_ENABLED
            and self.configuration.UMAMI_BASE_URL
            and self.configuration.UMAMI_WEBSITE_ID
        )

    @property
    def _api_configured(self) -> bool:
        return bool(
            self._collector_configured
            and self.configuration.UMAMI_API_USERNAME
            and self.configuration.UMAMI_API_PASSWORD
        )

    def _client(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(
            base_url=self.configuration.UMAMI_BASE_URL or "http://analytics.invalid",
            timeout=httpx.Timeout(self.configuration.ANALYTICS_REQUEST_TIMEOUT_SECONDS),
            transport=self.transport,
            headers={"User-Agent": "Verdaxis-Backend-Analytics/1.0"},
        )

    @staticmethod
    async def _bounded_json(response: httpx.Response) -> Any:
        content_length = response.headers.get("content-length")
        if content_length:
            try:
                if int(content_length) > _MAX_RESPONSE_BYTES:
                    raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            except ValueError:
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE) from None
        content = bytearray()
        try:
            async for chunk in response.aiter_bytes():
                content.extend(chunk)
                if len(content) > _MAX_RESPONSE_BYTES:
                    raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            return json.loads(content)
        except (UnicodeDecodeError, json.JSONDecodeError):
            raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE) from None

    async def _authenticate(self, *, force: bool = False) -> str:
        if not self._api_configured:
            diagnostic = (
                AnalyticsDiagnostic.DISABLED
                if not self.configuration.ANALYTICS_ENABLED
                else AnalyticsDiagnostic.CONFIGURATION
            )
            raise _AnalyticsError(diagnostic)

        now = time.monotonic()
        if not force and self._token and now < self._token_expires_at:
            return self._token

        async with self._token_lock:
            now = time.monotonic()
            if not force and self._token and now < self._token_expires_at:
                return self._token
            password = self.configuration.UMAMI_API_PASSWORD
            try:
                async with self._client() as client:
                    request = client.build_request(
                        "POST", "/api/auth/login", json={
                            "username": self.configuration.UMAMI_API_USERNAME,
                            "password": password.get_secret_value() if password else "",
                        },
                    )
                    response = await client.send(request, stream=True)
                    try:
                        if response.status_code in {401, 403}:
                            raise _AnalyticsError(AnalyticsDiagnostic.AUTHENTICATION)
                        if response.status_code >= 400:
                            raise _AnalyticsError(AnalyticsDiagnostic.UPSTREAM)
                        data = await self._bounded_json(response)
                    finally:
                        await response.aclose()
            except httpx.TimeoutException:
                raise _AnalyticsError(AnalyticsDiagnostic.TIMEOUT) from None
            except httpx.HTTPError:
                raise _AnalyticsError(AnalyticsDiagnostic.UPSTREAM) from None
            token = data.get("token") if isinstance(data, dict) else None
            if not isinstance(token, str) or not token or len(token) > 16_384:
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            self._token = token
            self._token_expires_at = time.monotonic() + _TOKEN_TTL_SECONDS
            return token

    async def _authorized_get(self, path: str, params: dict[str, Any]) -> Any:
        token = await self._authenticate()
        for attempt in range(2):
            try:
                async with self._client() as client:
                    request = client.build_request(
                        "GET", path, params=params,
                        headers={"Authorization": f"Bearer {token}"},
                    )
                    response = await client.send(request, stream=True)
                    try:
                        status_code = response.status_code
                        if status_code not in {401, 403} and status_code < 400:
                            return await self._bounded_json(response)
                    finally:
                        await response.aclose()
            except httpx.TimeoutException:
                raise _AnalyticsError(AnalyticsDiagnostic.TIMEOUT) from None
            except httpx.HTTPError:
                raise _AnalyticsError(AnalyticsDiagnostic.UPSTREAM) from None
            if status_code == 401 and attempt == 0:
                if self._token == token:
                    self._token = None
                token = await self._authenticate()
                continue
            if status_code in {401, 403}:
                raise _AnalyticsError(AnalyticsDiagnostic.AUTHENTICATION)
            if status_code >= 400:
                raise _AnalyticsError(AnalyticsDiagnostic.UPSTREAM)
        raise _AnalyticsError(AnalyticsDiagnostic.AUTHENTICATION)

    @staticmethod
    def _number(value: Any) -> int:
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
        return max(0, int(value))

    @classmethod
    def _metric_rows(cls, value: Any, limit: int = 10) -> list[dict[str, Any]]:
        if not isinstance(value, list):
            raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
        rows = []
        for item in value[:limit]:
            if not isinstance(item, dict) or not isinstance(item.get("x"), str):
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            rows.append({"name": item["x"][:500], "value": cls._number(item.get("y"))})
        return rows

    async def _fetch_event_property_rows(
        self, base: str, params: dict[str, Any], event_name: str
    ) -> list[dict[str, Any]]:
        """Per-value breakdown for one registered event.

        Uses the verified ``event-data/events?event=<name>`` form — the only
        property route shape supported by the installed Umami 3.2.0 (see
        docs/behavioral-analytics-contract.md). Rows are bounded and
        allowlist-shaped; anything else is a malformed response.
        """
        payload = await self._authorized_get(
            f"{base}/event-data/events", {**params, "event": event_name}
        )
        if not isinstance(payload, list):
            raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
        rows: list[dict[str, Any]] = []
        for item in payload[:_MAX_PROPERTY_ROWS]:
            if (
                not isinstance(item, dict)
                or not isinstance(item.get("propertyName"), str)
                or not isinstance(item.get("propertyValue"), str)
            ):
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            rows.append(
                {
                    "property": item["propertyName"][:120],
                    "value": item["propertyValue"][:500],
                    "total": self._number(item.get("total")),
                }
            )
        return rows

    async def _fetch_window(
        self,
        start: datetime,
        end: datetime,
        event_properties: tuple[str, ...],
    ) -> BehavioralWindowAggregate:
        website = self.configuration.UMAMI_WEBSITE_ID
        if not website:
            raise _AnalyticsError(AnalyticsDiagnostic.CONFIGURATION)
        params = {
            "startAt": int(start.timestamp() * 1000),
            "endAt": int(end.timestamp() * 1000),
        }
        base = f"/api/websites/{website}"
        # Authenticate once before fan-out so an unavailable auth endpoint is
        # hit once, not once per aggregate subrequest.
        await self._authenticate()
        stats, pageviews, event_series, entries, referrers = await asyncio.gather(
            self._authorized_get(f"{base}/stats", params),
            self._authorized_get(f"{base}/pageviews", {**params, "unit": "day", "timezone": "UTC"}),
            self._authorized_get(f"{base}/events/series", {**params, "unit": "day", "timezone": "UTC"}),
            self._authorized_get(f"{base}/metrics", {**params, "type": "entry", "limit": 10}),
            self._authorized_get(f"{base}/metrics", {**params, "type": "referrer", "limit": 10}),
        )
        if not isinstance(stats, dict) or not isinstance(pageviews, dict) or not isinstance(event_series, list):
            raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)

        event_totals: dict[str, int] = {}
        event_points: dict[tuple[str, str], int] = {}
        for item in event_series:
            if not isinstance(item, dict):
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            event_name = item.get("x")
            timestamp = item.get("t")
            if not isinstance(event_name, str) or not isinstance(timestamp, str):
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            if event_name in _ADMIN_REPORT_EVENT_NAMES:
                value = self._number(item.get("y"))
                date = timestamp[:10]
                event_totals[event_name] = event_totals.get(event_name, 0) + value
                event_points[(date, event_name)] = event_points.get((date, event_name), 0) + value
        filtered_event_series = [
            {"date": date, "event": event_name, "value": value}
            for (date, event_name), value in sorted(event_points.items())[-2500:]
        ]
        session_rows = pageviews.get("sessions")
        if not isinstance(session_rows, list):
            raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
        daily_visitors = []
        for item in session_rows[:92]:
            if not isinstance(item, dict) or not isinstance(item.get("x"), str):
                raise _AnalyticsError(AnalyticsDiagnostic.MALFORMED_RESPONSE)
            daily_visitors.append(
                {"date": item["x"][:10], "value": self._number(item.get("y"))}
            )

        # Per-event property breakdowns are best-effort: a failed breakdown
        # degrades the window to partial instead of failing the whole tab.
        property_rows: dict[str, list[dict[str, Any]]] = {}
        property_failures: list[str] = []
        failure_diagnostic: AnalyticsDiagnostic | None = None
        if event_properties:
            outcomes = await asyncio.gather(
                *(
                    self._fetch_event_property_rows(base, params, event_name)
                    for event_name in event_properties
                ),
                return_exceptions=True,
            )
            for event_name, outcome in zip(event_properties, outcomes):
                if isinstance(outcome, BaseException):
                    diagnostic = (
                        outcome.diagnostic
                        if isinstance(outcome, _AnalyticsError)
                        else AnalyticsDiagnostic.UPSTREAM
                    )
                    failure_diagnostic = failure_diagnostic or diagnostic
                    property_failures.append(event_name)
                    logger.warning(
                        "behavioral_analytics.property_breakdown_unavailable",
                        extra={"event": event_name, "diagnostic": diagnostic.value},
                    )
                else:
                    property_rows[event_name] = outcome

        return BehavioralWindowAggregate(
            status="partial" if property_failures else "available",
            diagnostic=failure_diagnostic,
            observed_at=datetime.now(UTC),
            start=start,
            end=end,
            visitors=self._number(stats.get("visitors")),
            visits=self._number(stats.get("visits")),
            pageviews=self._number(stats.get("pageviews")),
            total_time_seconds=self._number(stats.get("totaltime")),
            event_totals=event_totals,
            event_series=filtered_event_series,
            daily_visitors=daily_visitors,
            top_entries=self._metric_rows(entries),
            top_referrers=self._metric_rows(referrers),
            event_properties=property_rows,
            property_failures=tuple(property_failures),
        )

    @staticmethod
    def _normalized_window(start: datetime, end: datetime) -> tuple[datetime, datetime]:
        if start.tzinfo is None:
            start = start.replace(tzinfo=UTC)
        if end.tzinfo is None:
            end = end.replace(tzinfo=UTC)
        return start.astimezone(UTC), end.astimezone(UTC)

    async def get_window_aggregate(
        self,
        start: datetime,
        end: datetime,
        *,
        event_properties: tuple[str, ...] = (),
    ) -> BehavioralWindowAggregate:
        """Date-bounded aggregate with optional per-event property breakdowns.

        Results are cached in a bounded LRU keyed by the normalized window,
        the requested breakdown set, the website id, and a schema-version
        constant; identical concurrent requests share one upstream fetch.
        """
        start, end = self._normalized_window(start, end)
        if end <= start:
            raise ValueError("end must be after start")
        names = tuple(sorted(set(event_properties)))
        if len(names) > _MAX_PROPERTY_EVENTS:
            raise ValueError(f"at most {_MAX_PROPERTY_EVENTS} event breakdowns per request")
        registered = SERVER_EVENT_NAMES | FRONTEND_EVENT_NAMES
        for name in names:
            if name not in registered:
                raise ValueError(f"unregistered analytics event: {name}")

        key = (
            _WINDOW_CACHE_SCHEMA_VERSION,
            self.configuration.UMAMI_WEBSITE_ID,
            int(start.timestamp() * 1000),
            int(end.timestamp() * 1000),
            names,
        )
        cached = self._window_cache.get(key)
        if cached and time.monotonic() < cached[0]:
            self._window_cache.move_to_end(key)
            return cached[1]

        lock = self._window_locks.setdefault(key, asyncio.Lock())
        try:
            async with lock:
                cached = self._window_cache.get(key)
                if cached and time.monotonic() < cached[0]:
                    self._window_cache.move_to_end(key)
                    return cached[1]
                try:
                    result = await self._fetch_window(start, end, names)
                except _AnalyticsError as exc:
                    logger.warning(
                        "behavioral_analytics.window_unavailable",
                        extra={"diagnostic": exc.diagnostic.value},
                    )
                    result = BehavioralWindowAggregate.unavailable(exc.diagnostic, start, end)
                except Exception:
                    logger.warning(
                        "behavioral_analytics.window_unavailable",
                        extra={"diagnostic": AnalyticsDiagnostic.UPSTREAM.value},
                    )
                    result = BehavioralWindowAggregate.unavailable(
                        AnalyticsDiagnostic.UPSTREAM, start, end
                    )
                ttl = (
                    self.success_cache_seconds
                    if result.status == "available"
                    else self.failure_cache_seconds
                )
                self._window_cache[key] = (time.monotonic() + ttl, result)
                self._window_cache.move_to_end(key)
                while len(self._window_cache) > _WINDOW_CACHE_MAX_ENTRIES:
                    self._window_cache.popitem(last=False)
                return result
        finally:
            # Waiters keep their reference to the lock object; dropping the
            # dict entry only bounds the lock table.
            self._window_locks.pop(key, None)

    async def get_aggregate(self, days: int) -> BehavioralAggregate:
        if days not in {7, 30, 90}:
            raise ValueError("days must be 7, 30, or 90")
        # Quantize the rolling window to five-minute boundaries so repeated
        # dashboard polls share one LRU entry (preserving the legacy caching
        # behavior within the same TTL bound).
        quantized = int(time.time() // _LEGACY_WINDOW_QUANTUM_SECONDS) * (
            _LEGACY_WINDOW_QUANTUM_SECONDS
        )
        end = datetime.fromtimestamp(quantized, tz=UTC)
        start = end - timedelta(days=days)
        window = await self.get_window_aggregate(start, end)
        if window.status == "unavailable":
            diagnostic = window.diagnostic or AnalyticsDiagnostic.UPSTREAM
            return BehavioralAggregate.unavailable(diagnostic)
        return BehavioralAggregate(
            status="available",
            diagnostic=None,
            observed_at=window.observed_at,
            visitors=window.visitors,
            visits=window.visits,
            pageviews=window.pageviews,
            total_time_seconds=window.total_time_seconds,
            event_totals=window.event_totals,
            event_series=window.event_series,
            daily_visitors=window.daily_visitors,
            top_entries=window.top_entries,
            top_referrers=window.top_referrers,
        )

    async def send_event(self, event: AnalyticsEvent) -> bool:
        if not self._collector_configured:
            return False
        configured_hostname = urlparse(self.configuration.FRONTEND_URL).hostname
        payload = {
            "type": "event",
            "payload": {
                "website": self.configuration.UMAMI_WEBSITE_ID,
                "hostname": event.hostname or configured_hostname or "localhost",
                "url": "/server-events",
                "title": "Verdaxis server event",
                "name": event.name,
                "data": event.data(),
            },
        }
        try:
            async with self._client() as client:
                async with client.stream(
                    "POST",
                    "/api/send",
                    json=payload,
                    headers={"User-Agent": event.user_agent or _FALLBACK_BROWSER_USER_AGENT},
                ) as response:
                    if response.status_code >= 400:
                        logger.warning("behavioral_analytics.event_delivery_failed", extra={"event": event.name})
                        return False
                    result = await self._bounded_json(response)
                    if isinstance(result, dict) and result.get("beep") == "boop":
                        logger.warning("behavioral_analytics.event_dropped", extra={"event": event.name})
                        return False
            return isinstance(result, dict)
        except Exception:
            logger.warning("behavioral_analytics.event_delivery_failed", extra={"event": event.name})
            return False

    def schedule_event(self, event: AnalyticsEvent) -> asyncio.Task[None]:
        async def _deliver() -> None:
            try:
                await self.send_event(event)
            except Exception:
                logger.warning("behavioral_analytics.event_delivery_failed", extra={"event": event.name})

        task = asyncio.create_task(_deliver(), name=f"analytics:{event.name}")
        self._pending_tasks.add(task)
        task.add_done_callback(self._pending_tasks.discard)
        return task


analytics_service = UmamiAnalyticsService(settings)


def get_analytics_service() -> UmamiAnalyticsService:
    return analytics_service


def track_analytics_event(event: AnalyticsEvent, *, request: Any | None = None) -> None:
    """Schedule a post-commit event without adding latency to the request."""
    provided_monitor_token = getattr(request, "headers", {}).get("x-monitor-token") if request else None
    configured_monitor_token = settings.MONITOR_TOKEN
    if (
        provided_monitor_token
        and configured_monitor_token
        and secrets.compare_digest(provided_monitor_token, configured_monitor_token)
    ):
        return
    analytics_service.schedule_event(event)


def _enum_value(value: Any) -> str | None:
    if value is None:
        return None
    raw = getattr(value, "value", value)
    return str(raw)


def _bounded_user_agent(value: str | None) -> str | None:
    if not value:
        return None
    sanitized = "".join(character for character in value if 32 <= ord(character) < 127).strip()
    return sanitized[:256] or None


def _bounded_hostname(value: str | None) -> str | None:
    if not value:
        return None
    sanitized = value.strip().lower()[:253]
    if not sanitized or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for character in sanitized):
        return None
    return sanitized


def _request_metadata(request: Any | None) -> dict[str, str | None]:
    if request is None:
        return {"user_agent": None, "hostname": None}
    headers = getattr(request, "headers", {})
    url = getattr(request, "url", None)
    return {
        "user_agent": _bounded_user_agent(headers.get("user-agent")),
        "hostname": _bounded_hostname(getattr(url, "hostname", None)),
    }


def _market_side_for_role(role: Any) -> str | None:
    role_value = _enum_value(role)
    if role_value == "BUYER":
        return "BID"
    if role_value == "SUPPLIER":
        return "ASK"
    return None


def registration_completed_event(user: Any, *, request: Any | None = None) -> AnalyticsEvent:
    return AnalyticsEvent(
        name="registration_completed",
        role=_enum_value(user.role) or "UNKNOWN",
        **_request_metadata(request),
    )


def organization_created_event(user: Any, *, request: Any | None = None) -> AnalyticsEvent:
    return AnalyticsEvent(
        name="organization_created",
        role=_enum_value(user.role) or "UNKNOWN",
        **_request_metadata(request),
    )


def order_created_event(user: Any, order: Any, *, request: Any | None = None) -> AnalyticsEvent:
    return AnalyticsEvent(
        name="order_created",
        role=_enum_value(user.role) or "UNKNOWN",
        side=_enum_value(order.side),
        canonical_product=getattr(order, "market_product", None),
        delivery_point=getattr(order, "delivery_point_name", None),
        availability_window=getattr(order, "availability_window", None),
        **_request_metadata(request),
    )


def trade_created_event(
    user: Any,
    *,
    order: Any | None = None,
    side: str | None = None,
    canonical_product: str | None = None,
    delivery_point: str | None = None,
    availability_window: str | None = None,
    request: Any | None = None,
) -> AnalyticsEvent:
    return AnalyticsEvent(
        name="trade_created",
        role=_enum_value(user.role) or "UNKNOWN",
        side=side or _market_side_for_role(user.role) or (_enum_value(getattr(order, "side", None)) if order else None),
        canonical_product=canonical_product or (getattr(order, "market_product", None) if order else None),
        delivery_point=delivery_point or (getattr(order, "delivery_point_name", None) if order else None),
        availability_window=availability_window or (getattr(order, "availability_window", None) if order else None),
        **_request_metadata(request),
    )

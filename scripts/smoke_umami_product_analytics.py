#!/usr/bin/env python3
"""Read-only contract smoke for the Umami event-data routes Product Analytics
calls (fe/docs/plans/2026-07-15-product-analytics-workspace.md §2.3).

The script authenticates with the view-only Umami API credentials, probes each
route, validates the bounded response shape, and prints a redacted report
(schemas and row counts only — never event payload values, tokens, or
credentials). It exits nonzero when a required route is unsupported or returns
an unexpected shape.

Verified against the installed Umami 3.2.0 on 2026-07-15:

- ``GET event-data/properties?startAt&endAt`` →
  ``[{eventName, propertyName, dataType, total}]`` — the event×property
  inventory. SUPPORTED, required.
- ``GET event-data/events?startAt&endAt&event=<name>`` →
  ``[{eventName, propertyName, dataType, propertyValue, total}]`` — per-value
  breakdown for one event. SUPPORTED, required. The unfiltered form (no
  ``event`` parameter) returns HTTP 500 on this build and must not be called.
- ``GET event-data/values?startAt&endAt&event=<name>&propertyName=<prop>`` →
  ``[{value, total}]``. SUPPORTED, required. The filter parameter is ``event``;
  passing ``eventName`` instead is silently ignored and yields empty results.
- ``GET event-data-pivot?startAt&endAt&eventName=<name>`` → paginated envelope
  ``{count, data, isCapped, page, pageSize}``. Supported, but excluded from
  the implementation contract (events/values already provide value-level
  aggregates); probed as optional for drift detection only.

Configuration comes from the environment (source .env.analytics first):
ANALYTICS_ENABLED, UMAMI_BASE_URL, UMAMI_WEBSITE_ID, UMAMI_API_USERNAME,
UMAMI_API_PASSWORD. Non-loopback base URLs are refused unless
--allow-remote-readonly is passed explicitly.

The shape validators below are imported by
tests/unit/test_product_analytics_contract.py so the MockTransport fixtures
and the live smoke enforce identical contracts.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlparse

import httpx

_MAX_RESPONSE_BYTES = 1_000_000
_MAX_ROWS = 5_000
_LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1"}

# Fallback probe identifiers when the site has no recorded event properties
# yet. These come from the registered frontend event taxonomy
# (app/services/behavioral_analytics.py) — an empty result set for them is a
# supported response; only a 4xx/5xx marks the route unsupported.
_FALLBACK_EVENT_NAME = "platform_navigation"
_FALLBACK_PROPERTY_NAME = "destination"


class ContractViolation(Exception):
    """Raised when a payload does not match the frozen contract shape."""


# ---------------------------------------------------------------------------
# Shape validators (shared with unit tests)
# ---------------------------------------------------------------------------


def _validate_rows(
    payload: Any,
    *,
    required_keys: frozenset[str],
    optional_keys: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Validate a bounded list-of-flat-dicts payload; return a redacted summary."""
    if not isinstance(payload, list):
        raise ContractViolation(f"expected a JSON array, got {type(payload).__name__}")
    if len(payload) > _MAX_ROWS:
        raise ContractViolation(f"row count {len(payload)} exceeds bound {_MAX_ROWS}")
    allowed = required_keys | optional_keys
    key_sets: set[tuple[str, ...]] = set()
    type_map: dict[str, str] = {}
    for row in payload:
        if not isinstance(row, dict):
            raise ContractViolation("row is not an object")
        keys = frozenset(row)
        missing = required_keys - keys
        if missing:
            raise ContractViolation(f"row missing required keys: {sorted(missing)}")
        unexpected = keys - allowed
        if unexpected:
            raise ContractViolation(f"row has unexpected keys: {sorted(unexpected)}")
        for key, value in row.items():
            if isinstance(value, bool) or not isinstance(value, (str, int, float, type(None))):
                raise ContractViolation(f"non-scalar value for key {key!r}")
            type_map.setdefault(key, type(value).__name__)
        key_sets.add(tuple(sorted(keys)))
    return {
        "row_count": len(payload),
        "key_sets": sorted(",".join(keys) for keys in key_sets),
        "types": type_map,
    }


def validate_event_data_events(payload: Any) -> dict[str, Any]:
    """Per-value rows for one event (``?event=<name>`` filtered form)."""
    return _validate_rows(
        payload,
        required_keys=frozenset({"eventName", "propertyName", "propertyValue", "total"}),
        optional_keys=frozenset({"dataType"}),
    )


def validate_event_data_properties(payload: Any) -> dict[str, Any]:
    return _validate_rows(
        payload,
        required_keys=frozenset({"propertyName", "total"}),
        optional_keys=frozenset({"eventName", "dataType"}),
    )


def validate_event_data_values(payload: Any) -> dict[str, Any]:
    return _validate_rows(
        payload,
        required_keys=frozenset({"value", "total"}),
        optional_keys=frozenset(),
    )


def validate_event_data_pivot_envelope(payload: Any) -> dict[str, Any]:
    """Paginated pivot envelope — drift detection only, not implemented against."""
    if not isinstance(payload, dict):
        raise ContractViolation(f"expected a JSON object, got {type(payload).__name__}")
    required = {"count", "data", "page", "pageSize"}
    missing = required - payload.keys()
    if missing:
        raise ContractViolation(f"envelope missing keys: {sorted(missing)}")
    data = payload["data"]
    if not isinstance(data, list) or len(data) > _MAX_ROWS:
        raise ContractViolation("envelope data must be a bounded array")
    return {
        "envelope_keys": sorted(payload.keys()),
        "row_count": len(data),
    }


def ensure_permitted_base_url(base_url: str, *, allow_remote_readonly: bool) -> None:
    """Refuse non-loopback collectors unless the caller opts in explicitly."""
    parsed = urlparse(base_url)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ContractViolation("UMAMI_BASE_URL must be an absolute HTTP(S) URL")
    if parsed.username or parsed.password:
        raise ContractViolation("UMAMI_BASE_URL must not embed credentials")
    if parsed.hostname not in _LOOPBACK_HOSTS and not allow_remote_readonly:
        raise ContractViolation(
            "refusing non-loopback Umami URL without --allow-remote-readonly"
        )


# ---------------------------------------------------------------------------
# Probes
# ---------------------------------------------------------------------------


@dataclass
class RouteProbe:
    name: str
    path_template: str
    required: bool
    validator: Any
    # Ordered parameter variants; the first accepted variant is recorded.
    param_variants: tuple[tuple[str, ...], ...]


PROPOSED_ROUTES: tuple[RouteProbe, ...] = (
    RouteProbe(
        name="event-data/properties",
        path_template="/api/websites/{website_id}/event-data/properties",
        required=True,
        validator=validate_event_data_properties,
        param_variants=(("startAt", "endAt"),),
    ),
    RouteProbe(
        # Only the event-filtered form: the bare form 500s on Umami 3.2.0.
        name="event-data/events?event=",
        path_template="/api/websites/{website_id}/event-data/events",
        required=True,
        validator=validate_event_data_events,
        param_variants=(("startAt", "endAt", "event"),),
    ),
    RouteProbe(
        # The filter parameter is ``event``; ``eventName`` is silently ignored
        # by this route and yields empty results (verified 2026-07-15).
        name="event-data/values",
        path_template="/api/websites/{website_id}/event-data/values",
        required=True,
        validator=validate_event_data_values,
        param_variants=(("startAt", "endAt", "event", "propertyName"),),
    ),
    RouteProbe(
        # Supported but excluded from the implementation contract; probed so
        # upstream drift is visible in smoke output.
        name="event-data-pivot",
        path_template="/api/websites/{website_id}/event-data-pivot",
        required=False,
        validator=validate_event_data_pivot_envelope,
        param_variants=(("startAt", "endAt", "eventName"),),
    ),
)


def _bounded_json(response: httpx.Response) -> Any:
    if len(response.content) > _MAX_RESPONSE_BYTES:
        raise ContractViolation("response exceeds 1MB bound")
    try:
        return response.json()
    except ValueError as error:
        raise ContractViolation("response is not valid JSON") from error


def _login(client: httpx.Client, username: str, password: str) -> str:
    response = client.post("/api/auth/login", json={"username": username, "password": password})
    if response.status_code in {401, 403}:
        raise ContractViolation("authentication rejected (check view-only credentials)")
    if response.status_code >= 400:
        raise ContractViolation(f"login failed with status {response.status_code}")
    data = _bounded_json(response)
    token = data.get("token") if isinstance(data, dict) else None
    if not isinstance(token, str) or not token:
        raise ContractViolation("login response missing token")
    return token


def _probe_route(
    client: httpx.Client,
    token: str,
    probe: RouteProbe,
    website_id: str,
    base_params: dict[str, Any],
    identifier_params: dict[str, str],
) -> dict[str, Any]:
    headers = {"Authorization": f"Bearer {token}"}
    path = probe.path_template.format(website_id=website_id)
    attempts: list[dict[str, Any]] = []
    for variant in probe.param_variants:
        params: dict[str, Any] = {}
        for key in variant:
            if key in base_params:
                params[key] = base_params[key]
            else:
                params[key] = identifier_params.get(key, "")
        response = client.get(path, params=params, headers=headers)
        attempt: dict[str, Any] = {
            "params": sorted(variant),
            "status_code": response.status_code,
        }
        if response.status_code < 400:
            try:
                payload = _bounded_json(response)
                attempt["schema"] = probe.validator(payload)
                attempt["supported"] = True
                attempts.append(attempt)
                return {"route": probe.name, "supported": True, "attempts": attempts}
            except ContractViolation as violation:
                attempt["supported"] = False
                attempt["violation"] = str(violation)
        else:
            attempt["supported"] = False
        attempts.append(attempt)
    return {"route": probe.name, "supported": False, "attempts": attempts}


def _pick_identifier_params(properties_payload: list[dict[str, Any]] | None) -> dict[str, str]:
    """Choose a real (event, property) pair from the inventory when available."""
    if properties_payload:
        for row in properties_payload:
            event_name = row.get("eventName")
            property_name = row.get("propertyName")
            if isinstance(event_name, str) and isinstance(property_name, str):
                return {
                    "event": event_name,
                    "eventName": event_name,
                    "propertyName": property_name,
                }
    return {
        "event": _FALLBACK_EVENT_NAME,
        "eventName": _FALLBACK_EVENT_NAME,
        "propertyName": _FALLBACK_PROPERTY_NAME,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--days", type=int, default=30, help="probe window in days")
    parser.add_argument(
        "--allow-remote-readonly",
        action="store_true",
        help="permit a non-loopback UMAMI_BASE_URL (read-only probes only)",
    )
    args = parser.parse_args()

    enabled = os.environ.get("ANALYTICS_ENABLED", "").strip().lower() in {"1", "true", "yes"}
    base_url = os.environ.get("UMAMI_BASE_URL", "").strip()
    website_id = os.environ.get("UMAMI_WEBSITE_ID", "").strip()
    username = os.environ.get("UMAMI_API_USERNAME", "").strip()
    password = os.environ.get("UMAMI_API_PASSWORD", "")

    if not enabled:
        print("ANALYTICS_ENABLED is not true — nothing to smoke", file=sys.stderr)
        return 2
    if not (base_url and website_id and username and password):
        print("missing Umami configuration (source .env.analytics first)", file=sys.stderr)
        return 2
    try:
        ensure_permitted_base_url(base_url, allow_remote_readonly=args.allow_remote_readonly)
    except ContractViolation as violation:
        print(str(violation), file=sys.stderr)
        return 2

    end_ms = int(time.time() * 1000)
    start_ms = end_ms - args.days * 24 * 60 * 60 * 1000
    base_params = {"startAt": start_ms, "endAt": end_ms}

    report: dict[str, Any] = {
        "base_url_host": urlparse(base_url).hostname,
        "website_id_prefix": website_id[:8],
        "window_days": args.days,
        "routes": [],
    }

    try:
        with httpx.Client(base_url=base_url, timeout=10.0) as client:
            token = _login(client, username, password)
            headers = {"Authorization": f"Bearer {token}"}

            # Fetch the property inventory first so the events/values probes
            # can target a real recorded (event, property) pair.
            properties_payload: list[dict[str, Any]] | None = None
            inventory_response = client.get(
                f"/api/websites/{website_id}/event-data/properties",
                params=base_params,
                headers=headers,
            )
            if inventory_response.status_code < 400:
                candidate = _bounded_json(inventory_response)
                if isinstance(candidate, list):
                    properties_payload = [row for row in candidate if isinstance(row, dict)]
            identifier_params = _pick_identifier_params(properties_payload)

            for probe in PROPOSED_ROUTES:
                result = _probe_route(
                    client, token, probe, website_id, base_params, identifier_params
                )
                result["required"] = probe.required
                report["routes"].append(result)
    except ContractViolation as violation:
        print(str(violation), file=sys.stderr)
        return 1
    except httpx.HTTPError as error:
        print(f"transport failure: {type(error).__name__}", file=sys.stderr)
        return 1

    print(json.dumps(report, indent=2, sort_keys=True))
    unsupported_required = [
        route["route"]
        for route in report["routes"]
        if route["required"] and not route["supported"]
    ]
    if unsupported_required:
        print(
            f"unsupported required routes: {', '.join(unsupported_required)}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

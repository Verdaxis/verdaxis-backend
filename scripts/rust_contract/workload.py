"""Load and validate the committed Task 14a workload manifest.

The manifest fixes, before any measured run, the fixture cardinalities, the
distribution seed, the route and MCP-tool weights, the measured query cases,
and the expected response-size bands. Both the seeded differential simulation
and the latency/memory measurement read this one file, so the two runs compare
the same workload.

Nothing here contacts a host or a database. Invalid or missing manifest data
fails closed: a run that cannot state its own workload does not measure.
"""

from __future__ import annotations

import hashlib
import json
import random
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
MANIFEST_PATH = ROOT / "tests" / "rust_contract" / "workload.json"

REQUIRED_ROUTE_FIELDS = (
    "id",
    "method",
    "path",
    "measure_query",
    "band_bytes",
    "weight_baseline",
    "weight_representative",
)
REQUIRED_TOOL_FIELDS = ("id", "weight_baseline", "weight_representative", "body", "band_bytes")
DECLARED_SIZES = ("small", "large")


class WorkloadError(RuntimeError):
    """The workload manifest is missing, malformed, or inconsistent."""


def load(path: Path = MANIFEST_PATH) -> dict[str, Any]:
    """Read the manifest, refusing anything that cannot define a workload."""
    try:
        manifest = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise WorkloadError("the workload manifest is unreadable") from error
    validate(manifest)
    return manifest


def canonical_json(value: Any) -> str:
    """One canonical spelling so a digest never depends on key order."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def manifest_digest(manifest: dict[str, Any]) -> str:
    """The exact-bytes digest of the workload definition that was run."""
    return hashlib.sha256(canonical_json(manifest).encode()).hexdigest()


def validate(manifest: dict[str, Any]) -> None:
    """Refuse a manifest that could not describe a complete, honest run."""
    if not isinstance(manifest, dict) or manifest.get("manifest_version") != 1:
        raise WorkloadError("unsupported workload manifest version")
    if not isinstance(manifest.get("distribution_seed"), int):
        raise WorkloadError("the distribution seed must be an integer")
    sizes = manifest.get("sizes")
    if not isinstance(sizes, dict) or set(sizes) != set(DECLARED_SIZES):
        raise WorkloadError("the manifest must declare exactly the small and large sizes")
    for name, entry in sizes.items():
        if not isinstance(entry, dict) or set(entry) != {"orders", "trades", "news"}:
            raise WorkloadError(f"size {name!r} must declare orders, trades and news")
        if not all(isinstance(value, int) and value > 0 for value in entry.values()):
            raise WorkloadError(f"size {name!r} cardinalities must be positive integers")
        if not entry["orders"] >= 1000 or not entry["trades"] >= 1000:
            raise WorkloadError(f"size {name!r} is below the representative floor")

    routes = manifest.get("routes")
    if not isinstance(routes, list) or len(routes) != 27:
        raise WorkloadError("the manifest must declare 27 public data routes")
    seen: set[str] = set()
    for route in routes:
        if not isinstance(route, dict) or any(k not in route for k in REQUIRED_ROUTE_FIELDS):
            raise WorkloadError("a route row is missing a required field")
        if route["method"] != "GET":
            raise WorkloadError("measured routes are GET reads only")
        if not route["path"].startswith("/api/"):
            raise WorkloadError("measured routes are public /api reads only")
        if route["id"] in seen:
            raise WorkloadError(f"duplicate route id {route['id']!r}")
        seen.add(route["id"])
        band = route["band_bytes"]
        if not (isinstance(band, list) and len(band) == 2 and 0 <= band[0] <= band[1]):
            raise WorkloadError(f"route {route['id']!r} has an invalid size band")
        if route["weight_baseline"] < 1 or route["weight_representative"] < 1:
            raise WorkloadError(f"route {route['id']!r} has a non-positive weight")

    tools = manifest.get("tools")
    if not isinstance(tools, list) or len(tools) != 3:
        raise WorkloadError("the manifest must declare the three MCP tools")
    tool_ids = {tool["id"] for tool in tools}
    if tool_ids != {"list_markets", "get_market_slice", "plan_voyage_fuel"}:
        raise WorkloadError("the manifest must declare exactly the three MCP tools")

    if manifest.get("required_observations_per_route") != 1000:
        raise WorkloadError("the observation floor is 1,000 per measured route")
    if not manifest.get("rates_rps"):
        raise WorkloadError("the manifest must declare the offered load rates")


def routes(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    return list(manifest["routes"])


def tools(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    return list(manifest["tools"])


def sizes(manifest: dict[str, Any]) -> dict[str, dict[str, int]]:
    return dict(manifest["sizes"])


def route_by_id(manifest: dict[str, Any], route_id: str) -> dict[str, Any]:
    for route in manifest["routes"]:
        if route["id"] == route_id:
            return route
    raise WorkloadError(f"unknown route {route_id!r}")


def _weight(route: dict[str, Any], weight_key: str) -> int:
    return int(route[weight_key if weight_key in route else "weight_baseline"])


def total_weight(manifest: dict[str, Any], weight_key: str = "weight_baseline") -> int:
    """The declared weight total, routes and tools together."""
    entries = routes(manifest) + tools(manifest)
    return sum(_weight(entry, weight_key) for entry in entries)


def weighted_schedule(
    manifest: dict[str, Any], weight_key: str = "weight_baseline"
) -> list[tuple[str, str]]:
    """A deterministic shuffle of the workload, expanded by declared weight.

    Each returned pair is ``("route", id)`` or ``("tool", id)``. The order is
    fixed by the manifest's distribution seed, so both runtimes see the same
    request sequence and a repetition replays it exactly.
    """
    expanded: list[tuple[str, str]] = []
    for route in routes(manifest):
        expanded += [("route", route["id"])] * _weight(route, weight_key)
    for tool in tools(manifest):
        expanded += [("tool", tool["id"])] * _weight(tool, weight_key)
    random.Random(manifest["distribution_seed"]).shuffle(expanded)
    return expanded


def observations_needed(
    seconds: float, offered_rps: float, weight: int, weight_total: int
) -> float:
    """How many observations one route receives inside a offered-load window.

    The whole offered load is 1,000 requests per second scaled by the rate; a
    route whose declared weight is ``weight`` receives ``offered_rps * weight /
    weight_total`` requests per second.
    """
    if weight_total <= 0:
        raise WorkloadError("the declared weight total must be positive")
    return seconds * offered_rps * weight / weight_total


def seconds_for_observations(
    required: int, offered_rps: float, weight: int, weight_total: int
) -> float:
    """The offered-load seconds a route needs to reach ``required`` samples."""
    per_second = offered_rps * weight / weight_total
    if per_second <= 0:
        raise WorkloadError("a route with no offered traffic can never reach the floor")
    return required / per_second

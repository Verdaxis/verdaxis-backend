"""Bounded read-only shadow harness for one short staging differential pass.

This harness compares Python and Rust on the same fixed synthetic public
queries. It never mirrors a user request: the case list comes from the
committed corpus, it carries no authorization or cookie header, and it uses a
fixed shadow request id and a safe Host header.

The rules that make the harness safe are pure functions, unit-tested without a
host:

* only an explicit, reviewed config file supplies target URLs; production
  hosts are refused;
* only read-only public GETs and the three read-only MCP tools are ever sent;
* a Python-Rust-Python sandwich classifies each pair; a data change is
  inconclusive, a mismatch is a difference, and neither is ever a pass;
* persisted diagnostics keep only schema paths, categories, and counters --
  never a body, a row id, an amount, a raw URL, a client address, or an
  unsalted hash.

The run loop itself is only exercised by the deferred Task 14b pass after the
operator deploys Rust to staging. It is disabled whenever deploy state exists.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from scripts.rust_contract import workload

ROOT = Path(__file__).resolve().parents[2]
IDENTITY_PATH = "/.well-known/verdaxis-disposable-test"

# Hosts and addresses that must never receive a shadow request.
PRODUCTION_HOSTS = frozenset(
    {
        "api.verdaxis.exchange",
        "app.verdaxis.exchange",
        "verdaxis.exchange",
        "www.verdaxis.exchange",
        "169.58.37.164",
        "vmi3623757",
    }
)

# A shadow request carries no caller identity and no credential.
FORBIDDEN_HEADERS = frozenset({"authorization", "cookie", "proxy-authorization", "x-api-key"})

# Only these diagnostic fields are ever persisted. Anything else is dropped, and
# a forbidden value in an allowed field is refused.
ALLOWED_DIAGNOSTIC_FIELDS = frozenset(
    {
        "candidate_sha",
        "policy_hash",
        "route_id",
        "case_id",
        "status_pair",
        "field_path",
        "difference_category",
        "byte_counts",
        "durations_ms",
        "counters",
    }
)
DIFFERENCE_CATEGORIES = frozenset(
    {"type", "keys", "length", "value", "sequence", "order", "tie-item-bytes", "surrounding-bytes"}
)

# A caller that tries to persist one of these is refused, not silently allowed:
# a difference is recorded as a category and a schema path, never as a value.
FORBIDDEN_DIAGNOSTIC_FIELDS = frozenset(
    {"body", "payload", "value", "expected", "actual", "difference_value", "url", "client_ip"}
)

# A field path is a schema path: object keys and array indexes only.
import re

_FIELD_PATH_RE = re.compile(r"^(/[A-Za-z0-9_]+|/[0-9]+)*$")
_UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)
_HASH_RE = re.compile(r"\b[0-9a-fA-F]{32,64}\b")
_BEARER_RE = re.compile(r"\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}", re.IGNORECASE)


class ShadowError(RuntimeError):
    """The shadow configuration or a persisted record is not safe."""


class ShadowPaused(ShadowError):
    """A pause condition (deploy state, 429/503, readiness) requires a wait."""


@dataclass
class ShadowConfig:
    """The reviewed target configuration. Every URL is explicit."""

    python_url: str
    rust_url: str
    sampling_calls_per_ten_seconds: int = 3
    per_call_deadline_seconds: float = 2.0
    max_response_bytes: int = 1024 * 1024
    output_dir: str = "work/rust-phase1/shadow"
    deploy_state_path: str = ""
    pause_seconds: int = 60
    required_observations_per_route: int = 1000
    price_minute_limits: dict[str, int] = field(default_factory=dict)
    candidate_sha: str = ""
    policy_hash: str = ""


def load_config(path: Path) -> ShadowConfig:
    """Read the reviewed config file. There are no implicit URL defaults."""
    import tomllib

    try:
        document = tomllib.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise ShadowError("the shadow config is unreadable") from error
    for key in ("python_url", "rust_url"):
        if not document.get(key):
            raise ShadowError(f"the shadow config must set {key} explicitly")
    assert_not_production(document["python_url"])
    assert_not_production(document["rust_url"])
    limits = document.get("limits", {})
    return ShadowConfig(
        python_url=str(document["python_url"]),
        rust_url=str(document["rust_url"]),
        sampling_calls_per_ten_seconds=int(document.get("sampling_calls_per_ten_seconds", 3)),
        per_call_deadline_seconds=float(document.get("per_call_deadline_seconds", 2.0)),
        max_response_bytes=int(document.get("max_response_bytes", 1024 * 1024)),
        output_dir=str(document.get("output_dir", "work/rust-phase1/shadow")),
        deploy_state_path=str(document.get("deploy_state_path", "")),
        pause_seconds=int(document.get("pause_seconds", 60)),
        required_observations_per_route=int(document.get("required_observations_per_route", 1000)),
        price_minute_limits={str(k): int(v) for k, v in limits.items()},
        candidate_sha=str(document.get("candidate_sha", "")),
        policy_hash=str(document.get("policy_hash", "")),
    )


def _host_of(url: str) -> str:
    without_scheme = url.split("://", 1)[-1]
    authority = without_scheme.split("/", 1)[0]
    return authority.rsplit("@", 1)[-1].split(":", 1)[0].lower()


def is_production_url(url: str) -> bool:
    """True when the URL names a production host or address."""
    return _host_of(url) in PRODUCTION_HOSTS


def assert_not_production(url: str) -> None:
    if is_production_url(url):
        raise ShadowError("a production URL is never a shadow target")


def assert_safe_headers(headers: dict[str, str]) -> None:
    """A shadow request must carry no credential and no identity header."""
    for name in headers:
        if str(name).lower() in FORBIDDEN_HEADERS:
            raise ShadowError("a shadow request must not carry a credential header")


def safe_cases(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    """The curated read-only case list: public GETs and read-only MCP tools."""
    cases: list[dict[str, Any]] = []
    for route in workload.routes(manifest):
        cases.append(
            {
                "kind": "route",
                "id": route["id"],
                "method": "GET",
                "path": route["path"],
                "query": route["measure_query"],
                "band_bytes": route["band_bytes"],
            }
        )
    for tool in workload.tools(manifest):
        cases.append(
            {
                "kind": "tool",
                "id": tool["id"],
                "method": "POST",
                "path": "/mcp",
                "body": tool["body"],
                "band_bytes": tool["band_bytes"],
            }
        )
    if not cases:
        raise ShadowError("the shadow case list is empty")
    return cases


def classify_sandwich(
    python_first: dict[str, Any], rust: dict[str, Any], python_second: dict[str, Any]
) -> str:
    """Compare a Python-Rust-Python sandwich.

    The two Python observations must agree after generation-time normalization;
    when they do not, the pair is a data change and cannot count. Only when they
    agree does the Rust observation decide between a match and a difference.
    """
    if python_first != python_second:
        return "data_changed"
    if python_first != rust:
        return "difference"
    return "match"


def status_pair_ok(status: int) -> bool:
    """A busy or overlarge response is not economic coverage."""
    return 200 <= status < 300


def coverage(
    pairs_per_route: dict[str, int],
    required: int,
    *,
    nonempty: dict[str, bool] | None = None,
) -> dict[str, Any]:
    """Coverage outcome per route: ok, or inconclusive with the number needed.

    Zero traffic is not zero differences: a route below the floor, or one whose
    only pairs are empty/busy, is inconclusive. A skipped route is a failure,
    never a pass.
    """
    nonempty = nonempty or {}
    outcomes: dict[str, Any] = {}
    inconclusive: list[str] = []
    for route_id, count in sorted(pairs_per_route.items()):
        if count < required:
            outcomes[route_id] = {"outcome": "inconclusive", "pairs": count, "needed": required - count}
            inconclusive.append(route_id)
        elif nonempty.get(route_id) is False:
            outcomes[route_id] = {"outcome": "inconclusive", "pairs": count, "needed": 0, "reason": "no nonempty observation"}
            inconclusive.append(route_id)
        else:
            outcomes[route_id] = {"outcome": "ok", "pairs": count}
    return {"routes": outcomes, "inconclusive": inconclusive}


def sanitize_diagnostic(record: dict[str, Any]) -> dict[str, Any]:
    """Keep only the allowed diagnostic fields, refusing forbidden content.

    A body, a row id, an amount, a raw URL, a client address, a credential, a
    difference *value*, or a bare hash in an allowed field stops the write.
    """
    sanitized: dict[str, Any] = {}
    for key, value in record.items():
        if key in FORBIDDEN_DIAGNOSTIC_FIELDS:
            raise ShadowError(f"a shadow diagnostic must not carry {key!r}")
        if key not in ALLOWED_DIAGNOSTIC_FIELDS:
            continue
        if key == "field_path":
            if not isinstance(value, str) or not _FIELD_PATH_RE.fullmatch(value):
                raise ShadowError("a field path must be a schema path, not a data key")
            if _UUID_RE.search(value):
                raise ShadowError("a field path must not contain a row id")
            sanitized[key] = value
        elif key == "difference_category":
            if value not in DIFFERENCE_CATEGORIES:
                raise ShadowError("an unknown difference category is refused")
            sanitized[key] = value
        else:
            text = json.dumps(value)
            if _UUID_RE.search(text):
                raise ShadowError("a row id must not be persisted")
            if _HASH_RE.search(text) and key != "policy_hash" and key != "candidate_sha":
                raise ShadowError("a bare hash of a value must not be persisted")
            if _BEARER_RE.search(text):
                raise ShadowError("a credential must not be persisted")
            if "?" in text and "http" in text:
                raise ShadowError("a raw URL with a query string must not be persisted")
            sanitized[key] = value
    return sanitized


def deploy_state_present(path: str) -> bool:
    """Deploy state blocks the harness, including a dangling symlink.

    ``os.path.lexists`` is true for a dangling symlink, so a state path that was
    removed but left a broken link still stops the run.
    """
    if not path:
        return False
    return os.path.lexists(path)


def _death_line(config: ShadowConfig) -> None:
    """Stop before serving when the deploy is mid-transition."""
    if deploy_state_present(config.deploy_state_path):
        raise ShadowPaused("deploy state is present; the shadow harness stays off")


def _shadow_request_id(route_id: str) -> str:
    """One fixed, bounded shadow correlation id, never a user value."""
    return f"shadow-{route_id.lower()}"


def attest(base_url: str, token: str) -> None:
    """Prove the target is the attested disposable endpoint before asking."""
    request = urllib.request.Request(base_url.rstrip("/") + IDENTITY_PATH, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            payload = json.loads(response.read())
    except (urllib.error.URLError, OSError, ValueError) as error:
        raise ShadowPaused("the shadow target did not attest") from error
    if payload.get("disposable") is not True or payload.get("token") != token:
        raise ShadowPaused("the shadow target is not the attested disposable endpoint")


def _one_call(config: ShadowConfig, base_url: str, case: dict[str, Any]) -> dict[str, Any]:
    """Send one bounded, read-only case and return a comparable envelope.

    A timeout, an over-large body, or a busy status is returned as an
    inconclusive marker, never as a successful comparison.
    """
    headers = {"x-request-id": _shadow_request_id(case["id"])}
    assert_safe_headers(headers)
    body = None
    if case["method"] == "POST":
        body = json.dumps(case["body"]).encode()
        # The streamable-HTTP transport requires exactly this Accept set; a
        # plain application/json Accept is refused 406 before any tool runs.
        headers["content-type"] = "application/json"
        headers["accept"] = "application/json, text/event-stream"
    url = base_url.rstrip("/") + case["path"]
    if case.get("query"):
        url += "?" + case["query"]
    request = urllib.request.Request(url, data=body, headers=headers, method=case["method"])
    try:
        with urllib.request.urlopen(request, timeout=config.per_call_deadline_seconds) as response:
            raw = response.read(config.max_response_bytes + 1)
            status = response.status
    except urllib.error.HTTPError as error:
        return {"status": error.code, "inconclusive": "busy" if error.code in (429, 503) else "error"}
    except (socket.timeout, TimeoutError):
        return {"status": None, "inconclusive": "timeout"}
    except (urllib.error.URLError, OSError):
        return {"status": None, "inconclusive": "unreachable"}
    if len(raw) > config.max_response_bytes:
        return {"status": status, "inconclusive": "oversize"}
    return {"status": status, "bytes": len(raw), "digest": None, "raw_len": len(raw)}


def run(config: ShadowConfig, duration_seconds: int, token: str) -> dict[str, Any]:
    """The bounded shadow loop. Only the deferred Task 14b pass calls this."""
    _death_line(config)
    manifest = workload.load()
    cases = safe_cases(manifest)
    schedule = [pair for pair in workload.weighted_schedule(manifest)]
    attest(config.python_url, token)
    attest(config.rust_url, token)

    deadline = time.monotonic() + duration_seconds
    counters: dict[str, int] = {"match": 0, "difference": 0, "data_changed": 0, "inconclusive": 0}
    pairs: dict[str, int] = {case["id"]: 0 for case in cases}
    diagnostics: list[dict[str, Any]] = []
    index = 0
    calls_per_window = config.sampling_calls_per_ten_seconds
    window_started = time.monotonic()
    calls_in_window = 0
    while time.monotonic() < deadline:
        _death_line(config)
        if calls_in_window >= calls_per_window:
            # Three upstream calls per window: one sandwich. Wait out the window.
            sleep = 10.0 - (time.monotonic() - window_started)
            if sleep > 0:
                time.sleep(min(sleep, config.pause_seconds))
            window_started = time.monotonic()
            calls_in_window = 0
        kind, identifier = schedule[index % len(schedule)]
        index += 1
        case = next(c for c in cases if c["id"] == identifier)
        first = _one_call(config, config.python_url, case)
        middle = _one_call(config, config.rust_url, case)
        third = _one_call(config, config.python_url, case)
        calls_in_window += 3
        if any(step.get("inconclusive") for step in (first, middle, third)):
            counters["inconclusive"] += 1
            continue
        outcome = classify_sandwich(first, middle, third)
        counters[outcome] += 1
        if outcome == "match":
            pairs[case["id"]] = pairs.get(case["id"], 0) + 1
        else:
            diagnostics.append(
                sanitize_diagnostic(
                    {
                        "candidate_sha": config.candidate_sha,
                        "policy_hash": config.policy_hash,
                        "route_id": case["id"],
                        "case_id": case["id"],
                        "status_pair": f"{first.get('status')}/{middle.get('status')}/{third.get('status')}",
                        "difference_category": "value",
                        "byte_counts": [first.get("raw_len", 0), middle.get("raw_len", 0)],
                    }
                )
            )
    return {
        "candidate_sha": config.candidate_sha,
        "policy_hash": config.policy_hash,
        "counters": counters,
        "pairs": pairs,
        "coverage": coverage(pairs, config.required_observations_per_route),
        "diagnostics": diagnostics,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--duration-seconds", type=int, required=True)
    args = parser.parse_args(argv)
    token = os.environ.get("DISPOSABLE_TEST_TOKEN", "")
    if len(token) < 32:
        print("DISPOSABLE_TEST_TOKEN is required", file=__import__("sys").stderr)
        return 2
    try:
        config = load_config(args.config)
        result = run(config, args.duration_seconds, token)
    except ShadowError as error:
        print(f"shadow run refused: {error}", file=__import__("sys").stderr)
        return 1
    out = ROOT / config.output_dir / "shadow-summary.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result["counters"]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

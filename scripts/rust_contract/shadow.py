"""Bounded read-only shadow harness for one short staging differential pass.

This harness compares Python and Rust on the same fixed synthetic public
queries. It never mirrors a user request: the case list is operator-reviewed
and private, it carries no authorization or cookie header, and it uses a
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
import http.client
import json
import os
import signal
import socket
import time
from collections import deque
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from scripts.rust_contract import compare, workload

ROOT = Path(__file__).resolve().parents[2]

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
    python_sha: str = ""
    cases_path: str = ""
    floor_approved: bool = False
    max_evidence_bytes: int = 10 * 1024 * 1024


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
        python_sha=str(document.get("python_sha", "")),
        cases_path=str(document.get("cases_path", "")),
        floor_approved=document.get("floor_approved", False),
        max_evidence_bytes=int(document.get("max_evidence_bytes", 10 * 1024 * 1024)),
    )


MINUTE_LIMITS = {"prices_per_minute": 60, "reference_per_minute": 30, "export_per_minute": 10}


def validate_config(config: ShadowConfig) -> None:
    if (config.python_url != "http://127.0.0.1:8001"
            or config.rust_url != "http://127.0.0.1:8011"):
        raise ShadowError("staging targets must be the exact reviewed loopback URLs")
    if not (1 <= config.sampling_calls_per_ten_seconds <= 3
            and 0 < config.per_call_deadline_seconds <= 2
            and 0 < config.max_response_bytes <= 1024 * 1024
            and config.pause_seconds >= 60
            and config.required_observations_per_route > 0
            and 0 < config.max_evidence_bytes <= 10 * 1024 * 1024):
        raise ShadowError("unsafe staging limits")
    if not config.deploy_state_path or not Path(config.deploy_state_path).is_absolute():
        raise ShadowError("an absolute deploy state path is required")
    for sha in (config.python_sha, config.candidate_sha):
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            raise ShadowError("separate full readiness identities are required")
    if type(config.floor_approved) is not bool:
        raise ShadowError("floor approval must be explicit")
    for key, value in config.price_minute_limits.items():
        if key not in MINUTE_LIMITS or not 0 < value <= MINUTE_LIMITS[key]:
            raise ShadowError("unsafe minute limit")


def load_staging_cases(path: Path, *, require_complete=True) -> list[dict[str, Any]]:
    """Read private, operator-reviewed public queries, never disposable defaults."""
    try:
        cases = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise ShadowError("private staging cases are unreadable") from error
    routes = {r["id"]: r["path"] for r in workload.routes(workload.load())}
    tools = {t["id"] for t in workload.tools(workload.load())}
    seen = set()
    for case in cases:
        identifier = case.get("id", "")
        if identifier in seen or not re.fullmatch(r"(?:R\d{2}|list_markets|get_market_slice|plan_voyage_fuel)", identifier):
            raise ShadowError("invalid or duplicate case ID")
        seen.add(identifier)
        if not re.fullmatch(r"(?:R\d{2}|list_markets|get_market_slice|plan_voyage_fuel)-[a-z0-9-]{1,32}", case.get("case_id", "")):
            raise ShadowError("a fixed synthetic case ID is required")
        if set(case) - {"id", "case_id", "kind", "path", "method", "query", "body", "group"}:
            raise ShadowError("unexpected staging case field")
        if identifier in routes:
            expected = routes[identifier]
            valid_path = (case.get("path") == expected if identifier != "R02" else
                          bool(re.fullmatch(r"/api/ports/[A-Z]{5}", case.get("path", ""))))
            if not valid_path or case.get("method") != "GET":
                raise ShadowError("unreviewed public route")
        elif identifier in tools:
            body = case.get("body", {})
            if (case.get("path") != "/mcp" or case.get("method") != "POST"
                    or body.get("method") != "tools/call"
                    or body.get("params", {}).get("name") != identifier):
                raise ShadowError("only the three read-only tools are permitted")
        else:
            raise ShadowError("unknown staging route")
        if any(c in case.get("query", "") for c in ("#", "\r", "\n")):
            raise ShadowError("invalid query transport")
        number = int(identifier[1:]) if identifier.startswith("R") else 0
        case["group"] = ("mcp" if not number else "simple" if number <= 5 else
                         "prices" if number in {6, 12, 13, 14} else
                         "curves" if number <= 11 else "orderbook")
    if not cases or (require_complete and seen != set(routes) | tools):
        raise ShadowError("staging coverage must name all routes and tools")
    return cases


class CallPacer:
    """One rolling upstream budget, including probes and both oracle calls."""
    def __init__(self, *, clock=None, sleep=None, calls=3):
        self.clock = clock or time.monotonic
        self.sleep = sleep or time.sleep
        self.calls = calls
        self.global_calls = deque()
        self.minute_calls = {}

    def wait(self, target, bucket, limit, *, deadline):
        while True:
            now = self.clock()
            queues = [(self.global_calls, 10.0, self.calls)]
            if bucket:
                queues.append((self.minute_calls.setdefault((target, bucket), deque()), 60.0, limit))
            delay = 0.0
            for queue, window, ceiling in queues:
                while queue and queue[0] <= now - window:
                    queue.popleft()
                if len(queue) >= ceiling:
                    delay = max(delay, queue[0] + window - now)
            if now + delay >= deadline:
                raise ShadowPaused("comparison duration reached")
            if delay:
                self.sleep(delay)
                continue
            for queue, _, _ in queues:
                queue.append(now)
            return


def round_robin_cases(cases):
    groups = {}
    for case in cases:
        groups.setdefault(case["group"], deque()).append(case)
    schedule = []
    while any(groups.values()):
        for group in groups.values():
            if group:
                schedule.append(group.popleft())
    return schedule


def memory_pressure() -> bool:
    """Use the existing kernel pressure signal; no load measurement is run."""
    try:
        rows = Path("/proc/pressure/memory").read_text().splitlines()
        return any(row.startswith("full ") and float(row.split("avg10=", 1)[1].split()[0]) > 0
                   for row in rows)
    except (OSError, ValueError, IndexError) as error:
        raise ShadowError("memory pressure signal unavailable") from error


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
    python_first: dict[str, Any], rust: dict[str, Any], python_second: dict[str, Any],
    *, request_path: str,
) -> str:
    """Compare a Python-Rust-Python sandwich.

    The two Python observations must agree after generation-time normalization;
    when they do not, the pair is a data change and cannot count. Only when they
    agree does the Rust observation decide between a match and a difference.
    """
    if compare.compare_responses(python_first, python_second, request_path=request_path,
                                 profile="byte", live=True):
        return "data_changed"
    if compare.compare_responses(python_first, rust, request_path=request_path,
                                 profile="byte", live=True):
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
            schema_names = {"body", "headers", "status", "items", "summaries", "prices", "price_per_mt_usd",
                            "quantity_mt", "generated_at", "observed_at", "total", "content_type"}
            if any(not part.isdigit() and part not in schema_names for part in value.split("/")[1:]):
                raise ShadowError("a field path must contain only reviewed schema names")
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
            if key in {"route_id", "case_id"} and not re.fullmatch(
                r"(?:R\d{2}|READY|list_markets|get_market_slice|plan_voyage_fuel)(?:-[a-z0-9-]{1,32})?", str(value)
            ):
                raise ShadowError("only synthetic route and case IDs may be persisted")
            if key == "status_pair" and not re.fullmatch(r"(?:[1-5]\d\d|none)(?:/(?:[1-5]\d\d|none)){1,2}", str(value)):
                raise ShadowError("invalid status pair")
            if key in {"byte_counts", "durations_ms"} and (
                not isinstance(value, list) or len(value) > 3
                or any(type(v) not in {int, float} or not 0 <= v <= 1048576 for v in value)
            ):
                raise ShadowError("invalid numeric diagnostic")
            if key == "counters" and (not isinstance(value, dict) or any(
                k not in {"match", "difference", "data_changed", "inconclusive", "attempted", "busy", "timeout", "oversize", "unreachable", "error", "pauses"}
                or type(v) is not int or v < 0 for k, v in value.items()
            )):
                raise ShadowError("invalid diagnostic counters")
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


def _one_call(config: ShadowConfig, base_url: str, case: dict[str, Any]) -> dict[str, Any]:
    """Direct loopback HTTP, no proxy/redirect, bounded by a real elapsed alarm.

    The harness is serial and runs in the main thread on Linux. SIGALRM covers
    connect, headers and streamed body together, not just individual socket reads.
    No response, exception string, URL or header is written to disk here.
    """
    from urllib.parse import urlsplit

    target = urlsplit(base_url)
    started = time.monotonic()
    started_utc = datetime.now(timezone.utc).isoformat()
    headers = {"x-request-id": _shadow_request_id(case["id"]), "host": "127.0.0.1"}
    body = None
    if case["method"] == "POST":
        body = json.dumps(case["body"]).encode()
        headers.update({"content-type": "application/json", "accept": "application/json, text/event-stream"})
    assert_safe_headers(headers)
    path = case["path"] + ("?" + case["query"] if case.get("query") else "")
    connection = http.client.HTTPConnection(target.hostname, target.port, timeout=config.per_call_deadline_seconds)
    status = None
    def expired(*_):
        raise TimeoutError()
    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, config.per_call_deadline_seconds)
    try:
        connection.request(case["method"], path, body=body, headers=headers)
        response = connection.getresponse()
        status = response.status
        raw = response.read(config.max_response_bytes + 1)
        if len(raw) > config.max_response_bytes:
            return {"status": status, "inconclusive": "oversize"}
        result = {"status": status, "headers": dict(response.getheaders()), "raw": raw,
                  "bytes": len(raw), "duration_ms": (time.monotonic() - started) * 1000,
                  "started_utc": started_utc, "ended_utc": datetime.now(timezone.utc).isoformat()}
        if status in (429, 503):
            result["inconclusive"] = "busy"
        return result
    except TimeoutError:
        return {"status": status, "inconclusive": "timeout"}
    except (OSError, http.client.HTTPException):
        return {"status": status, "inconclusive": "unreachable"}
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)
        connection.close()


def _bucket(path):
    if path == "/api/prices/reference/export":
        return "export_per_minute"
    if path == "/api/prices/reference":
        return "reference_per_minute"
    if path == "/api/prices":
        return "prices_per_minute"
    return ""


def _nonempty(response, case):
    if response["status"] != 200:
        return False
    try:
        body = compare.parse_json(response["raw"])
    except compare.ContractError:
        return len(response["raw"].splitlines()) > 1  # CSV header alone is empty.
    if case["method"] == "POST":
        result = body.get("result", {}) if isinstance(body, dict) else {}
        return bool(result) and not result.get("isError", False) and "error" not in body
    if isinstance(body, list):
        return bool(body)
    if isinstance(body, dict):
        collections = [body[k] for k in ("items", "summaries", "prices", "curve", "rows", "ports", "bids", "asks") if k in body]
        return any(bool(v) for v in collections) if collections else bool(body)
    return False


def run(config: ShadowConfig, duration_seconds: int, *, cases=None) -> dict[str, Any]:
    """One serial, paced correctness pass. Never starts or seeds a service."""
    validate_config(config)
    if socket.gethostname().split(".", 1)[0] != "vmi1840561":
        raise ShadowError("the comparator must run on the staging host")
    if not 0 < duration_seconds <= 2100:
        raise ShadowError("comparison duration must be within the 35 minute cap")
    _death_line(config)
    cases = cases if cases is not None else load_staging_cases(Path(config.cases_path))
    if not cases:
        raise ShadowError("no staging cases")
    cases = round_robin_cases(cases)
    deadline = time.monotonic() + duration_seconds
    pacer = CallPacer(calls=config.sampling_calls_per_ten_seconds)
    counters = dict.fromkeys(("match", "difference", "data_changed", "inconclusive", "attempted", "busy", "timeout", "oversize", "unreachable", "error", "pauses"), 0)
    pairs = {c["id"]: 0 for c in cases}
    nonempty = {c["id"]: False for c in cases}
    diagnostics = []
    ready = {"id": "READY", "method": "GET", "path": "/health/ready"}
    limits = MINUTE_LIMITS | config.price_minute_limits

    def pause():
        counters["pauses"] += 1
        if time.monotonic() + config.pause_seconds >= deadline:
            raise ShadowPaused("comparison duration reached")
        time.sleep(config.pause_seconds)
        _death_line(config)

    def call(base, case):
        _death_line(config)
        while memory_pressure():
            pause()
        bucket = _bucket(case["path"])
        pacer.wait(base, bucket, limits.get(bucket, 60), deadline=deadline - config.per_call_deadline_seconds)
        _death_line(config)
        return _one_call(config, base, case)

    def readiness():
        for base, sha in ((config.python_url, config.python_sha), (config.rust_url, config.candidate_sha)):
            response = call(base, ready)
            if response.get("inconclusive") or response["status"] != 200:
                pause()
                raise ShadowError("readiness unavailable after safety pause")
            try:
                actual = compare.parse_json(response["raw"])
            except compare.ContractError:
                actual = None
            if actual != {"status": "ok", "db": "ok", "environment": "staging", "release_sha": sha}:
                raise ShadowError("readiness identity differs")

    failed = False
    final_ready = False
    try:
        readiness()
        for path, identifier in (("/health/live", "LIVE"), ("/health", "HEALTH")):
            for base, sha in ((config.python_url, config.python_sha), (config.rust_url, config.candidate_sha)):
                response = call(base, {"id": identifier, "method": "GET", "path": path})
                if response.get("inconclusive") or response["status"] != 200:
                    pause()
                    raise ShadowError("health contract unavailable")
                try:
                    actual = compare.parse_json(response["raw"])
                except compare.ContractError:
                    actual = None
                expected = ({"status": "ok"} if identifier == "LIVE" else
                            {"status": "ok", "db": "ok", "environment": "staging", "release_sha": sha})
                if actual != expected:
                    raise ShadowError("health identity differs")
        index = 0
        next_readiness = time.monotonic() + 60
        while time.monotonic() < deadline:
            if time.monotonic() >= next_readiness:
                readiness()
                next_readiness = time.monotonic() + 60
            case = cases[index % len(cases)]
            index += 1
            steps = []
            counters["attempted"] += 1
            for base in (config.python_url, config.rust_url, config.python_url):
                step = call(base, case)
                steps.append(step)
                if step.get("inconclusive"):
                    reason = step["inconclusive"]
                    counters[reason] += 1
                    counters["inconclusive"] += 1
                    if reason == "busy" or step.get("status") in (429, 503):
                        pause()
                    break
            if any(s.get("inconclusive") for s in steps):
                continue
            try:
                comparison_path = case["path"] + ("#" + case["id"] if case["method"] == "POST" else "")
                outcome = classify_sandwich(*steps, request_path=comparison_path)
            except compare.ContractError:
                counters["error"] += 1
                counters["inconclusive"] += 1
                continue
            counters[outcome] += 1
            successful = all(s["status"] == 200 for s in steps)
            if case["method"] == "POST":
                successful = successful and all(_nonempty(s, case) for s in steps)
            if outcome == "match" and successful:
                pairs[case["id"]] += 1
                nonempty[case["id"]] |= _nonempty(steps[0], case)
            elif outcome == "match":
                counters["inconclusive"] += 1
            if outcome == "difference":
                # A root schema path is intentionally conservative: dynamic JSON
                # keys and comparator hashes can never escape into diagnostics.
                diagnostics.append(sanitize_diagnostic({
                    "route_id": case["id"], "case_id": case["case_id"],
                    "status_pair": "/".join(str(s["status"]) for s in steps),
                    "field_path": "/body", "difference_category": "value",
                    "byte_counts": [s["bytes"] for s in steps],
                    "durations_ms": [s["duration_ms"] for s in steps],
                }))
                failed = True
                break
            if all(n >= config.required_observations_per_route for n in pairs.values()):
                readiness()  # Final identities share the same upstream budget.
                final_ready = True
                break
    except ShadowPaused:
        pass
    covered = coverage(pairs, config.required_observations_per_route, nonempty=nonempty)
    usable = sum(pairs.values())
    outcome = "FAIL" if failed else "INCONCLUSIVE"
    if (not failed and final_ready and config.floor_approved and not covered["inconclusive"]
            and counters["attempted"] and usable / counters["attempted"] >= .99):
        outcome = "PASS"
    return {"outcome": outcome, "counters": counters, "pairs": pairs,
            "coverage": covered, "diagnostics": diagnostics}


def result_exit_code(result):
    return {"PASS": 0, "FAIL": 1, "INCONCLUSIVE": 2}[result["outcome"]]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--duration-seconds", type=int, required=True)
    args = parser.parse_args(argv)
    try:
        config = load_config(args.config)
        output = Path(config.output_dir)
        if not output.is_absolute() or output.resolve().is_relative_to(ROOT.resolve()):
            raise ShadowError("evidence must be private and outside the source checkout")
        if output.exists() and (output.is_symlink() or output.stat().st_mode & 0o077):
            raise ShadowError("evidence directory must be private")
        result = run(config, args.duration_seconds)
        encoded = (json.dumps(result, indent=2) + "\n").encode()
        if len(encoded) > config.max_evidence_bytes:
            raise ShadowError("sanitized evidence limit exceeded")
        output = Path(config.output_dir)
        if not output.is_absolute() or output.is_relative_to(ROOT):
            raise ShadowError("evidence must be private and outside the source checkout")
        output.mkdir(mode=0o700, parents=True, exist_ok=True)
        out = output / "shadow-summary.json"
        # Refuse existing files and links; do not overwrite another run's evidence.
        fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(encoded)
    except (ShadowError, OSError, ValueError):
        print("shadow run refused; no response data retained", file=__import__("sys").stderr)
        return 1
    print(json.dumps(result["counters"]))
    return result_exit_code(result)


if __name__ == "__main__":
    raise SystemExit(main())

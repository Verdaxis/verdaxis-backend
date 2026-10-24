"""Record one HTTP exchange per corpus case against an attested endpoint.

A recording is a list of `{"id", "request", "exchange", "raw_body"}` entries.
The exchange is the compared envelope from `compare.capture`; `raw_body` keeps
the exact response bytes so the corpus can store `response.body` verbatim. The
endpoint is attested before any case is sent, so a record run can never point
at a live host.
"""

from __future__ import annotations

import argparse
import json
import os
import time
import urllib.error
import urllib.request
from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
from typing import Any

from scripts.rust_contract.compare import (
    CASES_PATH,
    SCHEMA_REVISION,
    ContractError,
    attest,
    capture,
    load_cases,
)

# One fixed correlation id per case, echoed back as the X-Request-ID contract
# header. A case may override it, but never with a value that fails the
# server's own bound.
DEFAULT_REQUEST_ID = "00000000-0000-4000-8000-000000000001"

# A finite HTTP body must finish inside this bound. MCP uses its protocol-aware
# reader below because a standalone GET stream has no terminal response.
CASE_DEADLINE_SECONDS = 30.0
CHUNK_BYTES = 65536

# A redirect's Location is resolved against the server the recorder talks to.
# That origin is swapped for one marker so the two endpoints stay comparable;
# a Location outside the request's own origin is left untouched and reported.
ENDPOINT_MARKER = "{endpoint}"


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


_OPENER = urllib.request.build_opener(_NoRedirect)


def _normalize_location(exchange: dict[str, Any], base_url: str) -> None:
    location = exchange["headers"].get("location")
    if location and location.startswith(base_url):
        exchange["headers"]["location"] = ENDPOINT_MARKER + location[len(base_url) :]


def _case_request_id(case: Mapping[str, Any]) -> str:
    return str(case.get("request_id") or DEFAULT_REQUEST_ID)


def _url(base_url: str, case: Mapping[str, Any]) -> str:
    path = str(case["path"])
    query = case.get("query")
    return base_url.rstrip("/") + path + (f"?{query}" if query else "")


class OracleUnavailable(ContractError):
    """The Python oracle exhausted its transient capture retries."""


def _transient_reason(error: BaseException) -> str | None:
    while error is not None:
        if isinstance(error, TimeoutError):
            return "capture timeout"
        if isinstance(error, ConnectionResetError):
            return "connection reset"
        if isinstance(error, urllib.error.URLError) and isinstance(error.reason, BaseException):
            error = error.reason
        else:
            error = error.__cause__
    return None


def _retry_after(entry: Mapping[str, Any]) -> float | None:
    if entry["exchange"]["status"] != 503:
        return None
    try:
        body = json.loads(entry["raw_body"])
    except (ValueError, UnicodeError):
        return None
    busy_details = (
        "temporarily busy",
        "Market is temporarily busy; retry shortly.",
        "Public market data is temporarily busy; retry shortly.",
    )
    if not any(body == {"detail": detail} for detail in busy_details):
        return None
    headers = entry.get("response_headers", {})
    value = next((str(value) for name, value in headers.items() if name.lower() == "retry-after"), "")
    if value.isascii() and value.isdigit():
        return float(value)
    try:
        date = parsedate_to_datetime(value)
        if date.tzinfo is None:
            return None
        return max(0.0, (date - datetime.now(timezone.utc)).total_seconds())
    except (TypeError, ValueError, OverflowError):
        return None


def _perform_oracle(
    base_url: str, case: Mapping[str, Any], *, group: str, leg: str, retries: list[dict[str, Any]],
) -> dict[str, Any]:
    # The first request plus at most three retries, never a whole-group replay.
    for attempt in range(4):
        try:
            entry = perform(base_url, case)
            delay = _retry_after(entry)
            if delay is None:
                return entry
            reason = "503 temporarily busy"
        except ContractError as error:
            reason = _transient_reason(error)
            if reason is None:
                raise
            delay = 0.0
        if attempt == 3:
            raise OracleUnavailable(f"ORACLE_UNAVAILABLE: {group}/{case['id']}: {reason}")
        event = {"group": group, "request": str(case["id"]), "leg": leg,
                 "reason": reason, "attempt": attempt + 1, "delay_seconds": delay}
        retries.append(event)
        evidence = os.environ.get("ORACLE_RETRY_LOG")
        if evidence:
            with Path(evidence).open("a") as stream:
                stream.write(json.dumps(event) + "\n")
        print("ORACLE_RETRY " + json.dumps(event), flush=True)
        time.sleep(delay)
    raise AssertionError("unreachable oracle attempt")


def _read_bounded(response) -> bytes:
    """Read a response body inside one wall-clock deadline.

    Each read is given only the time left before the deadline, so a stream that
    trickles forever is refused rather than followed.
    """
    deadline = time.monotonic() + CASE_DEADLINE_SECONDS
    chunks: list[bytes] = []
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ContractError("response exceeded the case deadline") from TimeoutError("case deadline")
        socket = getattr(getattr(response, "fp", None), "raw", None)
        socket = getattr(socket, "_sock", None)
        if socket is not None:
            try:
                socket.settimeout(remaining)
            except OSError:
                pass
        try:
            chunk = response.read(CHUNK_BYTES)
        except OSError as error:
            raise ContractError("response read failed") from error
        if not chunk:
            return b"".join(chunks)
        chunks.append(chunk)


def perform(base_url: str, case: Mapping[str, Any]) -> dict[str, Any]:
    """Send one case and return its recorded entry."""
    headers = {"x-request-id": _case_request_id(case)}
    for name, value in (case.get("headers") or {}).items():
        headers[str(name).lower()] = str(value)
    body = case.get("body")
    data = None if body is None else str(body).encode()
    request = urllib.request.Request(
        _url(base_url, case), data=data, headers=headers, method=str(case["method"])
    )

    def read_response(response):
        if case["path"] == "/mcp":
            from scripts.rust_contract.mcp_record import read_mcp_response

            return read_mcp_response(response, case)
        return _read_bounded(response)

    try:
        with _OPENER.open(request, timeout=CASE_DEADLINE_SECONDS) as response:
            raw = read_response(response)
            status = response.status
            response_headers = dict(response.headers.items())
    except urllib.error.HTTPError as error:
        # An error status, including a redirect the recorder does not follow,
        # is part of the contract, not a transport failure.
        # An incomplete error body is still a capture failure, never an answer.
        with error:
            raw = read_response(error)
        status = error.code
        response_headers = dict(error.headers.items())
    except (urllib.error.URLError, OSError) as error:
        raise ContractError(f"case {case['id']} could not be sent") from error
    exchange = capture(
        status,
        response_headers,
        raw,
        expect_body=str(case["method"]).upper() != "HEAD",
    )
    _normalize_location(exchange, base_url.rstrip("/"))
    return {
        "id": str(case["id"]),
        "request": {
            "method": str(case["method"]),
            "path": str(case["path"]),
            "query": case.get("query") or "",
            "headers": headers,
            **({"body": body} if case["path"] == "/mcp" and body is not None else {}),
        },
        "exchange": exchange,
        "raw_body": raw,
        "response_headers": response_headers,
    }


def record(
    base_url: str, token: str, cases: Sequence[Mapping[str, Any]],
    *, oracle: bool = False, group: str = "", leg: str = "", retries: list | None = None,
) -> list[dict[str, Any]]:
    """Attest, then capture in order; only an explicitly named oracle may retry.

    Child contract drivers inherit the exact oracle URL from Runner. Standalone
    and Rust recordings retain one-shot behavior by default.
    """
    if not cases:
        raise ContractError("no cases to record")
    attest(base_url, token)
    oracle = oracle or (
        os.environ.get("RUST_CONTRACT_PROFILE") == "phase1-meaning-v1"
        and os.environ.get("ORACLE_CAPTURE_URL") == base_url
    )
    retries = retries if retries is not None else []
    group = group or os.environ.get("ORACLE_CAPTURE_GROUP", "")
    leg = leg or os.environ.get("ORACLE_CAPTURE_LEG", "python")

    def capture_case(case):
        if oracle:
            return _perform_oracle(base_url, case, group=group, leg=leg, retries=retries)
        return perform(base_url, case)

    entries = []
    for case in cases:
        if case.get("fixture") == "T4B-negative-fame":
            import asyncio

            from sqlalchemy import text

            from app.seeds.safety import seed_session
            from scripts.rust_contract.seed_corpus import attest_session, matrix_module

            async def set_negative_status(status):
                async with seed_session() as session:
                    await attest_session(session)
                    await session.execute(
                        text("UPDATE orderbook_orders SET status=:status WHERE id=:id"),
                        {
                            "status": status,
                            "id": matrix_module().fid("order:typed_invalid_visible"),
                        },
                    )
                    await session.commit()

            asyncio.run(set_negative_status("OPEN"))
            try:
                entries.append(capture_case(case))
            finally:
                asyncio.run(set_negative_status("CANCELLED"))

        else:
            entries.append(capture_case(case))
    return entries


def stable(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Strip the raw bytes so a recording can be serialized and reloaded."""
    import base64
    return [
        {**{key: value for key, value in entry.items() if key != "raw_body"},
         **({"raw_body_base64": base64.b64encode(entry["raw_body"]).decode()}
            if os.environ.get("RUST_CONTRACT_PROFILE") == "phase1-meaning-v1" and "raw_body" in entry else {})}
        for entry in entries
    ]


def differences(first: list[dict[str, Any]], second: list[dict[str, Any]]) -> list[str]:
    """Compare two recordings of the same group. Empty means stable."""
    from scripts.rust_contract.compare import recording_differences

    return recording_differences(first, second, profile=os.environ.get("RUST_CONTRACT_PROFILE"),
                                 rules_sha256=os.environ.get("MEANING_RULES_SHA256"))


def write_golden(
    root: Path,
    source_sha: str,
    entries: Sequence[dict[str, Any]],
    *,
    meta: Mapping[str, Any],
) -> None:
    """Write the corpus under `golden/<source-sha>/<case-id>/`."""
    base = root / "golden" / source_sha
    for entry in entries:
        case_dir = base / entry["id"]
        case_dir.mkdir(parents=True, exist_ok=True)
        (case_dir / "request.json").write_text(
            json.dumps(entry["request"], indent=2) + "\n"
        )
        (case_dir / "response.json").write_text(
            json.dumps(entry["exchange"], indent=2, default=str) + "\n"
        )
        (case_dir / "response.body").write_bytes(entry["raw_body"])
        (case_dir / "meta.json").write_text(
            json.dumps({**meta, "case_id": entry["id"]}, indent=2) + "\n"
        )


def _main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--group", required=True)
    parser.add_argument("--cases", type=Path, default=CASES_PATH)
    parser.add_argument("--token-env", default="DISPOSABLE_TEST_TOKEN")
    parser.add_argument("--out", type=Path, help="write this recording as JSON")
    parser.add_argument(
        "--compare", type=Path, help="compare against an earlier recording"
    )
    parser.add_argument("--golden-root", type=Path, help="write the golden corpus here")
    parser.add_argument("--source-sha", default="")
    parser.add_argument("--timezone", default="")
    args = parser.parse_args(argv)
    token = os.environ.get(args.token_env, "")
    if not token:
        raise SystemExit(f"{args.token_env} is required")
    try:
        cases = load_cases(args.cases, args.group)
        entries = record(args.base_url, token, cases)
    except ContractError as error:
        print(f"recording failed: {error}")
        return 2
    if args.out:
        args.out.write_text(json.dumps(stable(entries), indent=2, default=str) + "\n")
    if args.golden_root:
        write_golden(
            args.golden_root,
            args.source_sha,
            entries,
            meta={
                "source_sha": args.source_sha,
                "schema_revision": SCHEMA_REVISION,
                "timezone": args.timezone,
                "group": args.group,
            },
        )
    if args.compare:
        earlier = json.loads(args.compare.read_text())
        problems = differences(earlier, stable(entries))
        for problem in problems:
            print(problem)
        if problems:
            print(f"recording is unstable: {len(problems)} difference(s)")
            return 1
        print(f"recording stable: {len(entries)} case(s)")
        return 0
    print(f"recorded {len(entries)} case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())

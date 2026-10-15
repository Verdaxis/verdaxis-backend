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
from pathlib import Path
from typing import Any

from scripts.rust_contract.compare import (
    CASES_PATH,
    SCHEMA_REVISION,
    ContractError,
    attest,
    capture,
    compare,
    load_cases,
)

# One fixed correlation id per case, echoed back as the X-Request-ID contract
# header. A case may override it, but never with a value that fails the
# server's own bound.
DEFAULT_REQUEST_ID = "00000000-0000-4000-8000-000000000001"

# A case must finish inside this bound. The MCP streamable transport holds an
# event stream open, so a plain read would never return; the recorder refuses
# it instead of hanging the whole run.
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
            raise ContractError("response exceeded the case deadline")
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
            raise ContractError("response read timed out") from error
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
    try:
        with _OPENER.open(request, timeout=CASE_DEADLINE_SECONDS) as response:
            raw = _read_bounded(response)
            status = response.status
            response_headers = dict(response.headers.items())
    except urllib.error.HTTPError as error:
        # An error status, including a redirect the recorder does not follow,
        # is part of the contract, not a transport failure.
        try:
            raw = _read_bounded(error)
        except ContractError:
            raw = b""
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
        },
        "exchange": exchange,
        "raw_body": raw,
    }


def record(
    base_url: str, token: str, cases: Sequence[Mapping[str, Any]]
) -> list[dict[str, Any]]:
    """Attest the endpoint, then record every case in order."""
    if not cases:
        raise ContractError("no cases to record")
    attest(base_url, token)
    entries = []
    for case in cases:
        if case.get("fixture") == "T4B-negative-fame":
            import asyncio
            from scripts.rust_contract.seed_corpus import matrix_module, attest_session
            from app.seeds.safety import seed_session
            from sqlalchemy import text

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
                entries.append(perform(base_url, case))
            finally:
                asyncio.run(set_negative_status("CANCELLED"))

        else:
            entries.append(perform(base_url, case))
    return entries


def stable(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Strip the raw bytes so a recording can be serialized and reloaded."""
    return [
        {key: value for key, value in entry.items() if key != "raw_body"}
        for entry in entries
    ]


def differences(first: list[dict[str, Any]], second: list[dict[str, Any]]) -> list[str]:
    """Compare two recordings of the same group. Empty means stable."""
    from scripts.rust_contract.compare import recording_differences

    return recording_differences(first, second)


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

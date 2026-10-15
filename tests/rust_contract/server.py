"""Serve the Python oracle with the disposable identity and a test-only echo.

This wrapper carries the unchanged application, the disposable identity
endpoint from `tests.disposable_server`, and one deterministic echo projection.
The echo route lets Task 4 prove the whole record/compare pipeline before any
ported data route exists; the Rust side serves the same projection.
"""

from __future__ import annotations

import argparse
import json
import os
from datetime import datetime
from urllib.parse import parse_qs

from tests.disposable_server import DisposableIdentityApp, validate_config

ECHO_PATH = "/__contract__/echo"

# The oracle serves every request at one explicit instant when the runner sets
# RUST_CONTRACT_CLOCK, so clock-dependent reads are reproducible. The Rust side
# must be given the same instant.
CLOCK_ENV = "RUST_CONTRACT_CLOCK"
_clock_stack = None


def apply_fixed_clock() -> None:
    """Freeze the module clocks for the process lifetime when requested."""
    global _clock_stack
    raw = os.environ.get(CLOCK_ENV, "")
    if not raw:
        return
    instant = datetime.fromisoformat(raw)
    if instant.tzinfo is None:
        raise SystemExit(f"{CLOCK_ENV} must be timezone-aware")
    from tests.rust_contract.clock import fixed_clock_patches

    _clock_stack = fixed_clock_patches(instant)


class ContractOracleApp:
    def __init__(self, application, token: str):
        self.identity = DisposableIdentityApp(application, token)

    async def __call__(self, scope, receive, send) -> None:
        if scope.get("type") == "http" and scope.get("path") == ECHO_PATH:
            await self._echo(scope, send)
            return
        await self.identity(scope, receive, send)

    async def _echo(self, scope, send) -> None:
        method = scope.get("method", "GET")
        request_id = next(
            (
                value.decode()
                for name, value in scope.get("headers", [])
                if name.lower() == b"x-request-id"
            ),
            "",
        )
        if method not in ("GET", "HEAD"):
            body = json.dumps({"detail": "Method Not Allowed"}, separators=(",", ":")).encode()
            headers = [
                (b"content-type", b"application/json"),
                (b"content-length", str(len(body)).encode()),
                (b"allow", b"GET, HEAD"),
            ]
        else:
            query = parse_qs(scope.get("query_string", b"").decode(), keep_blank_values=True)
            value = query.get("value", [None])[0]
            payload = json.dumps(
                {"echo": value, "method": method}, separators=(",", ":")
            ).encode()
            body = b"" if method == "HEAD" else payload
            headers = [
                (b"content-type", b"application/json"),
                (b"content-length", str(len(payload)).encode()),
            ]
            if request_id:
                # Mirror the ported server's correlation echo so the test-only
                # projection compares like the real middleware chain.
                headers.append((b"x-request-id", request_id.encode()))
        status = 200 if method in ("GET", "HEAD") else 405
        await send(
            {"type": "http.response.start", "status": status, "headers": headers}
        )
        await send({"type": "http.response.body", "body": body})


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args(argv)
    host = "127.0.0.1"
    token = validate_config(
        os.environ.get("ENVIRONMENT", ""),
        os.environ.get("DISPOSABLE_TEST_TOKEN", ""),
        host,
        args.port,
        os.environ.get("RELEASE_SHA", ""),
    )
    from app.main import app
    import uvicorn

    apply_fixed_clock()
    uvicorn.run(ContractOracleApp(app, token), host=host, port=args.port, log_level="warning")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

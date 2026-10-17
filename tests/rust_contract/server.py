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
        if scope.get("type") == "http" and scope.get("path") == "/__contract__/live-benchmarks":
            from tests.rust_contract.live_benchmarks import project

            query = parse_qs(scope.get("query_string", b"").decode())
            # Helper cases run sequentially. Rebind the entire clock seam, not
            # only expiry: it overrides explicit SQL and KYC clock arguments.
            global _clock_stack
            from tests.rust_contract.clock import fixed_clock_patches
            instant = datetime.fromisoformat(query.get("as_of", [os.environ[CLOCK_ENV]])[0])
            if _clock_stack is not None:
                _clock_stack.close()
            _clock_stack = fixed_clock_patches(instant)
            try:
                body = json.dumps(await project(query), separators=(",", ":")).encode()
            finally:
                _clock_stack.close()
                _clock_stack = None
                apply_fixed_clock()
            headers = [(b"content-type", b"application/json"), (b"content-length", str(len(body)).encode())]
            headers.extend((name, value) for name, value in scope.get("headers", []) if name == b"x-request-id")
            await send({"type": "http.response.start", "status": 200, "headers": headers})
            await send({"type": "http.response.body", "body": body})
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
    import uvicorn

    from app.main import app

    apply_fixed_clock()
    uvicorn.run(ContractOracleApp(app, token), host=host, port=args.port, log_level="warning")
    return 0


def factory():
    """ASGI factory for uvicorn's multi-worker mode.

    `uvicorn tests.rust_contract.server:factory --factory --workers 4` runs the
    four-worker Python service the measurement compares against. The workers
    share one socket through uvicorn's own multiprocess parent, exactly like the
    deployed `--workers 4` unit. The fixed clock is applied per worker process.
    """
    from app.main import app

    token = validate_config(
        os.environ.get("ENVIRONMENT", ""),
        os.environ.get("DISPOSABLE_TEST_TOKEN", ""),
        "127.0.0.1",
        int(os.environ.get("UVICORN_PORT", "0") or 0) or _factory_port(),
        os.environ.get("RELEASE_SHA", ""),
    )
    apply_fixed_clock()
    return ContractOracleApp(app, token)


def _factory_port() -> int:
    """The port a factory-started worker listens on.

    The disposable guard requires a numeric-loopback ephemeral port. Under
    `--workers`, uvicorn's parent owns the socket, so the port is the value the
    caller assigned; there is no implicit default.
    """
    value = os.environ.get("UVICORN_BIND_PORT", "")
    if not value:
        raise SystemExit("UVICORN_BIND_PORT is required for the factory service")
    return int(value)


if __name__ == "__main__":
    raise SystemExit(main())

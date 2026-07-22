"""Run the Verdaxis ASGI app with a test-only disposable-target attestation."""

from __future__ import annotations

import argparse
import json
import os
import re
from collections.abc import Awaitable, Callable

from tests.disposable_target import (
    MAX_DISPOSABLE_PORT,
    MIN_DISPOSABLE_PORT,
    PURPOSE,
    TOKEN_PATTERN,
)


IDENTITY_PATH = "/.well-known/verdaxis-disposable-test"

# The integration suite's readiness assertions require a real commit
# identity: RELEASE_SHA=$(git rev-parse HEAD). The unit-test default
# ("test") would make /health/ready fail confusingly mid-suite, so the
# producer refuses to start without a full 40-hex SHA.
RELEASE_SHA_PATTERN = re.compile(r"[0-9a-f]{40}")


class ServerConfigError(ValueError):
    """The disposable server configuration could reach a non-test boundary."""


def validate_config(
    environment: str, token: str, host: str, port: int, release_sha: str = ""
) -> str:
    if environment != "test":
        raise ServerConfigError("disposable server requires ENVIRONMENT=test")
    if TOKEN_PATTERN.fullmatch(token or "") is None:
        raise ServerConfigError("disposable server requires a bounded token")
    if host != "127.0.0.1" or not MIN_DISPOSABLE_PORT <= port <= MAX_DISPOSABLE_PORT:
        raise ServerConfigError("disposable server requires numeric loopback ephemeral bind")
    if RELEASE_SHA_PATTERN.fullmatch(release_sha or "") is None:
        raise ServerConfigError(
            "disposable server requires RELEASE_SHA=$(git rev-parse HEAD) (full 40-hex)"
        )
    return token


class DisposableIdentityApp:
    def __init__(self, application: Callable[..., Awaitable[None]], token: str):
        self.application = application
        self.body = json.dumps(
            {"disposable": True, "purpose": PURPOSE, "token": token},
            separators=(",", ":"),
        ).encode()

    async def __call__(self, scope, receive, send) -> None:
        if scope.get("type") != "http" or scope.get("path") != IDENTITY_PATH:
            await self.application(scope, receive, send)
            return
        status = 200 if scope.get("method") == "GET" else 405
        body = self.body if status == 200 else b""
        await send(
            {
                "type": "http.response.start",
                "status": status,
                "headers": [
                    (b"content-type", b"application/json"),
                    (b"content-length", str(len(body)).encode()),
                    (b"cache-control", b"no-store"),
                ],
            }
        )
        await send({"type": "http.response.body", "body": body})


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
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

    uvicorn.run(DisposableIdentityApp(app, token), host=host, port=args.port)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

"""Fail-closed target validation shared by integration and E2E tests."""

from __future__ import annotations

import http.client
import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Mapping
from urllib.parse import urlsplit


IDENTITY_PATH = "/.well-known/verdaxis-disposable-test"
IDENTITY_KEYS = frozenset({"disposable", "purpose", "token"})
PURPOSE = "verdaxis-integration-test"
TOKEN_PATTERN = re.compile(r"[A-Za-z0-9._~-]{32,256}")
MIN_DISPOSABLE_PORT = 49152
MAX_DISPOSABLE_PORT = 65535


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict:
    payload = {}
    for key, value in pairs:
        if key in payload:
            raise ValueError("duplicate JSON key")
        payload[key] = value
    return payload


class DisposableTargetError(ValueError):
    """The requested integration target is not provably disposable."""


@dataclass(frozen=True)
class DisposableTarget:
    base_url: str
    token: str
    port: int

    @property
    def identity_url(self) -> str:
        return f"{self.base_url}{IDENTITY_PATH}"


def validate_target(url: str | None, token: str | None) -> DisposableTarget:
    if not isinstance(url, str) or not url:
        raise DisposableTargetError("an explicit disposable target URL is required")
    if not isinstance(token, str) or TOKEN_PATTERN.fullmatch(token) is None:
        raise DisposableTargetError("a bounded disposable target token is required")
    try:
        parsed = urlsplit(url)
        port = parsed.port
    except ValueError:
        raise DisposableTargetError("invalid disposable target URL") from None
    if (
        parsed.scheme != "http"
        or parsed.hostname != "127.0.0.1"
        or parsed.username is not None
        or parsed.password is not None
        or port is None
        or not MIN_DISPOSABLE_PORT <= port <= MAX_DISPOSABLE_PORT
        or parsed.path not in {"", "/"}
        or parsed.query
        or parsed.fragment
    ):
        raise DisposableTargetError(
            "disposable targets must use numeric loopback HTTP and an ephemeral port"
        )
    return DisposableTarget(base_url=f"http://127.0.0.1:{port}", token=token, port=port)


def from_environment(environ: Mapping[str, str] | None = None) -> DisposableTarget:
    values = os.environ if environ is None else environ
    if values.get("VERDAXIS_RUN_DISPOSABLE_INTEGRATION") != "1":
        raise DisposableTargetError("explicit disposable target opt-in is required")
    url = values.get("VERDAXIS_DISPOSABLE_TARGET_URL") or values.get("TEST_API_URL")
    return validate_target(url, values.get("VERDAXIS_DISPOSABLE_TARGET_TOKEN"))


def validate_identity_payload(raw: bytes, target: DisposableTarget) -> bool:
    if len(raw) > 1024:
        raise DisposableTargetError("disposable identity response is too large")
    try:
        payload = json.loads(raw, object_pairs_hook=_reject_duplicate_keys)
    except (UnicodeDecodeError, ValueError, RecursionError):
        raise DisposableTargetError("disposable identity response is invalid") from None
    if not isinstance(payload, dict) or set(payload) != IDENTITY_KEYS:
        raise DisposableTargetError("disposable identity schema mismatch")
    if (
        payload.get("disposable") is not True
        or payload.get("purpose") != PURPOSE
        or payload.get("token") != target.token
        or not isinstance(payload.get("purpose"), str)
        or not isinstance(payload.get("token"), str)
    ):
        raise DisposableTargetError("disposable identity attestation mismatch")
    return True


def _fetch_identity(url: str, *, timeout_seconds: float, max_bytes: int) -> bytes:
    parsed = urlsplit(url)
    if (
        parsed.scheme != "http"
        or parsed.hostname != "127.0.0.1"
        or parsed.path != IDENTITY_PATH
        or parsed.port is None
    ):
        raise DisposableTargetError("identity fetch target changed after validation")
    connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=timeout_seconds)
    try:
        connection.request(
            "GET",
            IDENTITY_PATH,
            headers={"Accept": "application/json", "Connection": "close"},
        )
        response = connection.getresponse()
        if response.status != 200:
            raise DisposableTargetError("disposable identity endpoint rejected attestation")
        if response.getheader("Transfer-Encoding") is not None:
            raise DisposableTargetError("chunked disposable identity responses are rejected")
        length = response.getheader("Content-Length")
        if length is None or not length.isdigit() or int(length) > max_bytes:
            raise DisposableTargetError("invalid disposable identity response length")
        raw = response.read(max_bytes + 1)
        if len(raw) != int(length) or len(raw) > max_bytes:
            raise DisposableTargetError("invalid disposable identity response length")
        return raw
    except (OSError, http.client.HTTPException) as exc:
        raise DisposableTargetError("disposable identity endpoint is unavailable") from exc
    finally:
        connection.close()


def attest(
    target: DisposableTarget,
    *,
    fetch: Callable[..., bytes] = _fetch_identity,
) -> None:
    raw = fetch(target.identity_url, timeout_seconds=2.0, max_bytes=1024)
    validate_identity_payload(raw, target)


def requires_disposable_target(path: Path) -> bool:
    normalized = tuple(part.lower() for part in path.parts)
    return (
        "integration" in normalized
        or "e2e" in normalized
        or path.name.lower().startswith("e2e_")
        or path.name.lower().startswith("test_e2e")
    )

#!/usr/bin/env python3
"""Credential-free local readiness and filesystem checks."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import stat
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Iterable
from urllib.parse import urlsplit


MODULE_DIRECTORY = str(Path(__file__).resolve().parent)
if MODULE_DIRECTORY not in sys.path:
    sys.path.insert(0, MODULE_DIRECTORY)

from status_state import (  # noqa: E402 - installed sibling module
    atomic_write_status,
    build_status,
    failure_status,
    load_monitor_status,
    quarantine,
    reject_duplicate_keys,
)


ENDPOINTS = (
    ("prod_ready", "http://127.0.0.1:8000/health/ready"),
    ("staging_ready", "http://127.0.0.1:8001/health/ready"),
)
EXPECTED_ENVIRONMENTS = {"prod_ready": "production", "staging_ready": "staging"}
RELEASE_SHA_PATTERN = re.compile(r"[0-9a-f]{40}")
IDENTITY_KEYS = frozenset({"ENVIRONMENT", "RELEASE_SHA"})
READINESS_KEYS = frozenset({"status", "db", "environment", "release_sha"})
MAX_IDENTITY_BYTES = 256


def _duration_ms(started: float) -> int:
    return max(0, round((time.monotonic() - started) * 1000))


def _result(name: str, category: str, code: str, started: float) -> dict:
    return {
        "name": name,
        "category": category,
        "code": code,
        "duration_ms": _duration_ms(started),
    }


def validate_runtime_identities(identities: dict[str, tuple[str | None, str | None]]) -> None:
    if set(identities) != set(EXPECTED_ENVIRONMENTS):
        raise ValueError("runtime identity inventory must remain exact")
    for name, expected_environment in EXPECTED_ENVIRONMENTS.items():
        environment, release_sha = identities[name]
        if environment != expected_environment:
            raise ValueError("runtime environment does not match endpoint")
        if (
            not isinstance(release_sha, str)
            or RELEASE_SHA_PATTERN.fullmatch(release_sha) is None
            or release_sha == "0" * 40
        ):
            raise ValueError("runtime release identity must be a full Git SHA")


def _read_identity(path: Path, expected_environment: str) -> tuple[str, str]:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise ValueError("runtime identity file is invalid")
            raw = handle.read(MAX_IDENTITY_BYTES + 1)
    except (OSError, ValueError) as exc:
        raise ValueError("runtime identity file is invalid") from exc
    if len(raw) > MAX_IDENTITY_BYTES or not raw.endswith(b"\n"):
        raise ValueError("runtime identity file is invalid")
    try:
        lines = raw.decode("ascii").splitlines()
    except UnicodeDecodeError as exc:
        raise ValueError("runtime identity file is invalid") from exc
    if len(lines) != 2:
        raise ValueError("runtime identity file is invalid")
    values: dict[str, str] = {}
    for line in lines:
        if line.count("=") != 1:
            raise ValueError("runtime identity file is invalid")
        key, value = line.split("=", 1)
        if key not in IDENTITY_KEYS or key in values or not value:
            raise ValueError("runtime identity file is invalid")
        values[key] = value
    if set(values) != IDENTITY_KEYS:
        raise ValueError("runtime identity file is invalid")
    environment = values["ENVIRONMENT"]
    release_sha = values["RELEASE_SHA"]
    if environment != expected_environment:
        raise ValueError("runtime identity file is invalid")
    if (
        RELEASE_SHA_PATTERN.fullmatch(release_sha) is None
        or release_sha == "0" * 40
    ):
        raise ValueError("runtime identity file is invalid")
    return environment, release_sha


def load_runtime_identities(production_file: Path, staging_file: Path) -> dict[str, tuple[str, str]]:
    production = _read_identity(production_file, "production")
    staging = _read_identity(staging_file, "staging")
    identities = {
        "prod_ready": production,
        "staging_ready": staging,
    }
    validate_runtime_identities(identities)
    return identities


def validate_readiness_payload(
    payload: object,
    expected_environment: str,
    expected_release_sha: str,
) -> None:
    if (
        expected_environment not in {"production", "staging"}
        or RELEASE_SHA_PATTERN.fullmatch(expected_release_sha) is None
        or expected_release_sha == "0" * 40
    ):
        raise ValueError("expected runtime identity is invalid")
    if not isinstance(payload, dict) or set(payload) != READINESS_KEYS:
        raise ValueError("readiness schema mismatch")
    if any(not isinstance(payload[key], str) for key in READINESS_KEYS):
        raise ValueError("readiness schema mismatch")
    if payload["status"] != "ok" or payload["db"] != "ok":
        raise ValueError("readiness is unhealthy")
    if (
        payload["environment"] != expected_environment
        or payload["release_sha"] != expected_release_sha
    ):
        raise ValueError("readiness identity mismatch")


def _remaining(deadline: float) -> float:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError
    return remaining


def _read_http_response(url: str, deadline: float, max_body_bytes: int, max_header_bytes: int = 16_384):
    parsed = urlsplit(url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port:
        raise ValueError("endpoint must be numeric loopback HTTP")
    target = parsed.path or "/"
    if parsed.query:
        target += f"?{parsed.query}"
    request = (
        f"GET {target} HTTP/1.1\r\n"
        f"Host: 127.0.0.1:{parsed.port}\r\n"
        "Accept: application/json\r\n"
        "Connection: close\r\n\r\n"
    ).encode("ascii")
    data = bytearray()
    header_end = -1
    content_length = None
    with socket.create_connection((parsed.hostname, parsed.port), timeout=_remaining(deadline)) as connection:
        connection.settimeout(_remaining(deadline))
        connection.sendall(request)
        while True:
            connection.settimeout(_remaining(deadline))
            chunk = connection.recv(4096)
            if not chunk:
                break
            data.extend(chunk)
            if header_end < 0:
                header_end = data.find(b"\r\n\r\n")
                if header_end < 0:
                    if len(data) > max_header_bytes:
                        raise ValueError("headers_too_large")
                    continue
                if header_end > max_header_bytes:
                    raise ValueError("headers_too_large")
                header_lines = bytes(data[:header_end]).split(b"\r\n")
                try:
                    _protocol, status_raw, _reason = header_lines[0].split(b" ", 2)
                    status = int(status_raw)
                except (ValueError, IndexError):
                    raise ValueError("invalid_http") from None
                headers = {}
                for line in header_lines[1:]:
                    if b":" not in line:
                        raise ValueError("invalid_http")
                    key, value = line.split(b":", 1)
                    normalized = key.strip().lower()
                    if normalized in headers:
                        raise ValueError("invalid_http")
                    headers[normalized] = value.strip()
                if b"transfer-encoding" in headers:
                    raise ValueError("unsupported_transfer_encoding")
                if b"content-length" in headers:
                    try:
                        content_length = int(headers[b"content-length"])
                    except ValueError:
                        raise ValueError("invalid_http") from None
                    if content_length < 0:
                        raise ValueError("invalid_http")
                    if content_length > max_body_bytes:
                        raise ValueError("body_too_large")
            if header_end >= 0:
                body_length = len(data) - header_end - 4
                if body_length > max_body_bytes:
                    raise ValueError("body_too_large")
                if content_length is not None and body_length >= content_length:
                    break
    if header_end < 0:
        raise ValueError("invalid_http")
    body = bytes(data[header_end + 4 :])
    if content_length is not None:
        if len(body) != content_length:
            raise ValueError("invalid_http")
    return status, body


def check_readiness(
    name: str,
    url: str,
    expected_environment: str,
    expected_release_sha: str,
    timeout_seconds: float,
    max_body_bytes: int,
) -> dict:
    started = time.monotonic()
    deadline = started + timeout_seconds
    try:
        status, body = _read_http_response(url, deadline, max_body_bytes)
    except (socket.timeout, TimeoutError):
        return _result(name, "failure", "timeout", started)
    except ValueError as exc:
        code = str(exc) if str(exc) in {
            "body_too_large",
            "headers_too_large",
            "unsupported_transfer_encoding",
        } else "connection_error"
        return _result(name, "failure", code, started)
    except OSError:
        return _result(name, "failure", "connection_error", started)

    if 300 <= status < 400:
        return _result(name, "failure", "redirect_rejected", started)
    if status != 200:
        return _result(name, "failure", "http_status", started)
    try:
        payload = json.loads(body, object_pairs_hook=reject_duplicate_keys)
    except (UnicodeDecodeError, ValueError, RecursionError):
        return _result(name, "failure", "invalid_json", started)
    try:
        validate_readiness_payload(payload, expected_environment, expected_release_sha)
    except ValueError as exc:
        code = "identity_mismatch" if str(exc) == "readiness identity mismatch" else "unexpected_json"
        return _result(name, "failure", code, started)
    return _result(name, "healthy", "ok", started)


def check_filesystem(
    name: str,
    path: str | Path,
    warning_free_percent: float,
    critical_free_percent: float,
    *,
    disk_usage: Callable = shutil.disk_usage,
) -> dict:
    started = time.monotonic()
    if warning_free_percent <= critical_free_percent:
        return _result(name, "failure", "invalid_thresholds", started)
    try:
        total, _used, free = disk_usage(path)
    except OSError:
        return _result(name, "failure", "filesystem_unreadable", started)
    free_percent = (free / total * 100) if total else 0.0
    if free_percent <= critical_free_percent:
        return _result(name, "failure", "disk_free_critical", started)
    if free_percent <= warning_free_percent:
        return _result(name, "warning", "disk_free_warning", started)
    return _result(name, "healthy", "ok", started)


def check_directory_size(
    name: str,
    path: str | Path,
    max_bytes: int,
    deadline: float,
    *,
    max_entries: int = 100_000,
) -> dict:
    started = time.monotonic()
    total = 0
    entries = 0
    pending = [Path(path)]
    try:
        while pending:
            if time.monotonic() >= deadline:
                return _result(name, "failure", "total_deadline_exceeded", started)
            current = pending.pop()
            with os.scandir(current) as iterator:
                for entry in iterator:
                    entries += 1
                    if entries > max_entries:
                        return _result(name, "failure", "directory_entry_limit", started)
                    if entry.is_dir(follow_symlinks=False):
                        pending.append(Path(entry.path))
                    elif entry.is_file(follow_symlinks=False):
                        total += entry.stat(follow_symlinks=False).st_size
                        if total > max_bytes:
                            return _result(name, "failure", "directory_size_exceeded", started)
    except OSError:
        return _result(name, "failure", "directory_unreadable", started)
    return _result(name, "healthy", "ok", started)


def execute_checks(
    checks: Iterable[Callable[[float], dict]],
    total_timeout_seconds: float,
    *,
    clock: Callable[[], float] = time.monotonic,
) -> list[dict]:
    deadline = clock() + total_timeout_seconds
    results = []
    for index, check in enumerate(checks, start=1):
        remaining = deadline - clock()
        if remaining <= 0:
            results.append(
                {
                    "name": f"check_{index}",
                    "category": "failure",
                    "code": "total_deadline_exceeded",
                    "duration_ms": 0,
                }
            )
            break
        try:
            results.append(check(remaining))
        except Exception:
            results.append(
                {
                    "name": f"check_{index}",
                    "category": "failure",
                    "code": "unexpected_error",
                    "duration_ms": 0,
                }
            )
    return results


def run_monitor(args: argparse.Namespace) -> dict:
    output = Path(args.status_file)
    generated_at = datetime.now(timezone.utc)
    previous, malformed = load_monitor_status(output)
    if malformed:
        quarantine(output)
        return failure_status("previous_status_invalid", generated_at)
    identities = load_runtime_identities(
        Path(args.production_identity_file),
        Path(args.staging_identity_file),
    )
    deadline = time.monotonic() + args.total_timeout
    checks = [
        lambda remaining, name=name, url=url, identity=identities[name]: check_readiness(
            name,
            url,
            identity[0],
            identity[1],
            min(args.request_timeout, remaining),
            args.max_body_bytes,
        )
        for name, url in ENDPOINTS
    ]
    checks.append(
        lambda _remaining: check_filesystem(
            "root_disk",
            args.filesystem,
            args.warning_free_percent,
            args.critical_free_percent,
        )
    )
    if args.analytics_directory:
        checks.append(
            lambda _remaining: check_directory_size(
                "analytics_directory",
                args.analytics_directory,
                args.analytics_max_bytes,
                deadline,
                max_entries=args.analytics_max_entries,
            )
        )
    results = execute_checks(checks, max(0.001, deadline - time.monotonic()))
    return build_status(results, previous, generated_at)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--status-file", default="/var/lib/verdaxis-health/status.json")
    parser.add_argument(
        "--production-identity-file",
        default="/etc/verdaxis-monitor/runtime-identities/production.identity",
    )
    parser.add_argument(
        "--staging-identity-file",
        default="/etc/verdaxis-monitor/runtime-identities/staging.identity",
    )
    parser.add_argument("--filesystem", default="/")
    parser.add_argument("--warning-free-percent", type=float, default=20.0)
    parser.add_argument("--critical-free-percent", type=float, default=15.0)
    parser.add_argument("--analytics-directory")
    parser.add_argument("--analytics-max-bytes", type=int, default=5 * 1024**3)
    parser.add_argument("--analytics-max-entries", type=int, default=100_000)
    parser.add_argument("--request-timeout", type=float, default=5.0)
    parser.add_argument("--total-timeout", type=float, default=20.0)
    parser.add_argument("--max-body-bytes", type=int, default=1024)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    output = Path(args.status_file)
    try:
        payload = run_monitor(args)
    except Exception:
        payload = failure_status("fatal_error", datetime.now(timezone.utc))
    atomic_write_status(output, payload)
    print(f"Verdaxis local health: {payload['category']}")
    return 0 if payload["category"] == "healthy" else 2


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Bounded hostile-input handling for local monitor JSON state."""

from __future__ import annotations

import errno
import json
import os
import stat
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


STATUS_KEYS = frozenset({"schema_version", "generated_at", "category", "checks"})
CHECK_KEYS = frozenset(
    {"name", "category", "code", "duration_ms", "last_success_at"}
)
CATEGORIES = frozenset({"healthy", "warning", "failure"})
CATEGORY_RANK = {"healthy": 0, "warning": 1, "failure": 2}


@dataclass(frozen=True)
class JsonLoad:
    value: object | None
    exists: bool
    malformed: bool


def reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict:
    payload = {}
    for key, value in pairs:
        if key in payload:
            raise ValueError("duplicate JSON key")
        payload[key] = value
    return payload


def _regular_json(path: Path, max_bytes: int) -> JsonLoad:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except FileNotFoundError:
        return JsonLoad(None, False, False)
    except OSError as exc:
        exists = exc.errno in {errno.ELOOP, errno.EACCES, errno.EPERM} or path.exists()
        return JsonLoad(None, exists, True)
    try:
        with os.fdopen(descriptor, "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                return JsonLoad(None, True, True)
            raw = handle.read(max_bytes + 1)
    except OSError:
        return JsonLoad(None, True, True)
    if len(raw) > max_bytes:
        return JsonLoad(None, True, True)
    try:
        return JsonLoad(
            json.loads(raw, object_pairs_hook=reject_duplicate_keys), True, False
        )
    except (UnicodeDecodeError, ValueError, RecursionError):
        return JsonLoad(None, True, True)


def load_json(path: Path, max_bytes: int = 65_536) -> JsonLoad:
    return _regular_json(path, max_bytes)


def _parse_utc(value: object) -> datetime | None:
    if not isinstance(value, str) or len(value) > 40:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed.astimezone(timezone.utc) if parsed.tzinfo is not None else None


def valid_monitor_status(value: object) -> bool:
    if not isinstance(value, dict) or set(value) != STATUS_KEYS:
        return False
    if type(value.get("schema_version")) is not int or value["schema_version"] != 1:
        return False
    category = value.get("category")
    if (
        not isinstance(category, str)
        or category not in CATEGORIES
        or _parse_utc(value.get("generated_at")) is None
    ):
        return False
    checks = value.get("checks")
    if not isinstance(checks, list) or not checks or len(checks) > 128:
        return False
    names: set[str] = set()
    for check in checks:
        if not isinstance(check, dict) or set(check) != CHECK_KEYS:
            return False
        name = check.get("name")
        code = check.get("code")
        duration = check.get("duration_ms")
        success = check.get("last_success_at")
        check_category = check.get("category")
        if (
            not isinstance(name, str)
            or not name
            or len(name) > 128
            or name in names
            or not isinstance(check_category, str)
            or check_category not in CATEGORIES
            or not isinstance(code, str)
            or not code
            or len(code) > 128
            or type(duration) is not int
            or not 0 <= duration <= 86_400_000
            or (success is not None and _parse_utc(success) is None)
        ):
            return False
        names.add(name)
    worst = max(
        (check["category"] for check in checks),
        key=CATEGORY_RANK.__getitem__,
    )
    return category == worst


def load_monitor_status(path: Path, max_bytes: int = 65_536) -> tuple[dict, bool]:
    loaded = load_json(path, max_bytes)
    if not loaded.exists:
        return {}, False
    if loaded.malformed or not valid_monitor_status(loaded.value):
        return {}, True
    return loaded.value, False


def quarantine(path: Path) -> Path | None:
    try:
        path.lstat()
    except FileNotFoundError:
        return None
    destination = path.with_name(f"{path.name}.invalid-{time.time_ns()}")
    os.replace(path, destination)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_CLOEXEC)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    return destination


def utc(value: datetime) -> str:
    return value.astimezone(timezone.utc).isoformat(timespec="seconds").replace(
        "+00:00", "Z"
    )


def failure_status(code: str, generated_at: datetime, *, name: str = "monitor") -> dict:
    now = utc(generated_at)
    return {
        "schema_version": 1,
        "generated_at": now,
        "category": "failure",
        "checks": [
            {
                "name": name,
                "category": "failure",
                "code": code,
                "duration_ms": 0,
                "last_success_at": None,
            }
        ],
    }


def build_status(checks: list[dict], previous: dict, generated_at: datetime) -> dict:
    now = utc(generated_at)
    previous_checks = previous.get("checks", []) if isinstance(previous, dict) else []
    if not isinstance(previous_checks, list):
        previous_checks = []
    previous_successes = {
        check.get("name"): check.get("last_success_at")
        for check in previous_checks
        if isinstance(check, dict)
    }
    clean_checks = []
    for check in checks:
        category = check["category"]
        clean_checks.append(
            {
                "name": check["name"],
                "category": category,
                "code": check["code"],
                "duration_ms": max(0, int(check["duration_ms"])),
                "last_success_at": (
                    now
                    if category == "healthy"
                    else previous_successes.get(check["name"])
                ),
            }
        )
    if not clean_checks:
        return failure_status("no_checks", generated_at)
    category = max(
        (item["category"] for item in clean_checks),
        key=CATEGORY_RANK.__getitem__,
    )
    return {
        "schema_version": 1,
        "generated_at": now,
        "category": category,
        "checks": clean_checks,
    }


def atomic_write_status(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor, temporary = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, sort_keys=True, separators=(",", ":"))
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_CLOEXEC)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass

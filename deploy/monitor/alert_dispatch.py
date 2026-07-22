#!/usr/bin/env python3
"""Deliver sanitized local-monitor transitions without exposing secrets to readers."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import secrets
import sys
import tempfile
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping


MODULE_DIRECTORY = str(Path(__file__).resolve().parent)
if MODULE_DIRECTORY not in sys.path:
    sys.path.insert(0, MODULE_DIRECTORY)

from status_state import load_json, quarantine  # noqa: E402 - installed sibling module


CHECKS = frozenset({"health", "backup"})
STATES = frozenset({"healthy", "failure"})
DESTINATIONS = frozenset({"telegram", "healthchecks"})
STATE_KEYS = frozenset({"schema_version", "state", "observed_at", "receipts"})
RECEIPT_KEYS = frozenset({"receipt_id", "delivered_at"})
HOURLY_REPEAT_SECONDS = 3600


def _utc(value: datetime) -> str:
    return value.astimezone(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def _parse_utc(value) -> datetime | None:
    if not isinstance(value, str) or len(value) > 40:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed.astimezone(timezone.utc) if parsed.tzinfo else None


def valid_alert_state(payload: object, now: datetime) -> bool:
    if not isinstance(payload, dict) or set(payload) != STATE_KEYS:
        return False
    if type(payload.get("schema_version")) is not int or payload["schema_version"] != 3:
        return False
    state = payload.get("state")
    if not isinstance(state, str) or state not in STATES:
        return False
    observed_at = _parse_utc(payload.get("observed_at"))
    if (
        observed_at is None
        or now.tzinfo is None
        or (observed_at - now.astimezone(timezone.utc)).total_seconds() > 300
    ):
        return False
    receipts = payload.get("receipts")
    if not isinstance(receipts, dict) or not set(receipts).issubset(DESTINATIONS):
        return False
    for destination, destination_receipts in receipts.items():
        if not isinstance(destination, str):
            return False
        if (
            not isinstance(destination_receipts, dict)
            or not destination_receipts
            or not set(destination_receipts).issubset(STATES)
        ):
            return False
        for receipt_state, receipt in destination_receipts.items():
            if not isinstance(receipt_state, str):
                return False
            if not isinstance(receipt, dict) or set(receipt) != RECEIPT_KEYS:
                return False
            receipt_id = receipt.get("receipt_id")
            if (
                not isinstance(receipt_id, str)
                or len(receipt_id) != 32
                or any(character not in "0123456789abcdef" for character in receipt_id)
            ):
                return False
            delivered_at = _parse_utc(receipt.get("delivered_at"))
            if delivered_at is None or delivered_at > observed_at:
                return False
    return True


def _load_state(path: Path, now: datetime) -> tuple[dict, bool, bool]:
    loaded = load_json(path, 16_384)
    if not loaded.exists:
        return {}, False, False
    if loaded.malformed or not valid_alert_state(loaded.value, now):
        quarantine(path)
        return {}, True, True
    return loaded.value, False, True


def _write_state(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
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


def _post(url: str, payload: dict | None, timeout_seconds: float = 10.0) -> None:
    body = json.dumps(payload).encode() if payload is not None else b""
    headers = {"Content-Type": "application/json"} if payload is not None else {}
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
        if response.status < 200 or response.status >= 300:
            raise OSError("delivery rejected")
        response.read(1024)


def _event_parts(event: str) -> tuple[str, str]:
    try:
        check, state = event.rsplit("-", 1)
    except ValueError:
        raise ValueError("invalid alert event") from None
    if check not in CHECKS or state not in STATES:
        raise ValueError("invalid alert event")
    return check, state


def _receipt_time(receipt: dict | None) -> datetime | None:
    return _parse_utc(receipt.get("delivered_at")) if receipt else None


def _delivery_kind(destination_receipts: dict | None, state: str) -> str:
    destination_receipts = destination_receipts or {}
    failure_at = _receipt_time(destination_receipts.get("failure"))
    healthy_at = _receipt_time(destination_receipts.get("healthy"))
    if state == "healthy":
        return "RECOVERY" if failure_at is not None else "HEALTHY"
    if failure_at is not None and (healthy_at is None or failure_at > healthy_at):
        return "REMINDER"
    return "FAILURE"


def _receipt_due(
    destination_receipts: dict | None,
    state: str,
    now: datetime,
) -> bool:
    destination_receipts = destination_receipts or {}
    failure_at = _receipt_time(destination_receipts.get("failure"))
    healthy_at = _receipt_time(destination_receipts.get("healthy"))
    if state == "healthy":
        return failure_at is not None and (
            healthy_at is None or failure_at > healthy_at
        )
    if failure_at is None or (healthy_at is not None and healthy_at >= failure_at):
        return True
    return (
        now.astimezone(timezone.utc) - failure_at
    ).total_seconds() >= HOURLY_REPEAT_SECONDS


def _state_payload(state: str, now: datetime, receipts: dict) -> dict:
    return {
        "schema_version": 3,
        "state": state,
        "observed_at": _utc(now),
        "receipts": receipts,
    }


def _process_event_locked(
    event: str,
    state_directory: Path,
    now: datetime,
    environ: Mapping[str, str],
) -> str:
    check, state = _event_parts(event)
    state_file = state_directory / f"{check}.json"
    previous, _malformed, _existed = _load_state(state_file, now)
    previous_receipts = previous.get("receipts", {})
    receipts = {
        destination: dict(destination_receipts)
        for destination, destination_receipts in previous_receipts.items()
    }

    healthchecks_url = environ.get(f"HEALTHCHECKS_{check.upper()}_URL", "").strip()
    telegram_token = environ.get("TELEGRAM_BOT_TOKEN", "").strip()
    telegram_chat = environ.get("TELEGRAM_CHAT_ID", "").strip()
    telegram_base = environ.get("TELEGRAM_API_BASE", "https://api.telegram.org").rstrip("/")
    telegram_configured = bool(telegram_token and telegram_chat)
    if not healthchecks_url and not telegram_configured:
        _write_state(state_file, _state_payload(state, now, receipts))
        return "delivery_unconfigured"

    due = {
        "healthchecks": bool(healthchecks_url)
        and _receipt_due(previous_receipts.get("healthchecks"), state, now),
        "telegram": telegram_configured
        and _receipt_due(previous_receipts.get("telegram"), state, now),
    }
    _write_state(state_file, _state_payload(state, now, receipts))
    delivery_failed = False
    delivered = False
    if due["healthchecks"]:
        target = healthchecks_url.rstrip("/") + ("/fail" if state == "failure" else "")
        try:
            _post(target, None)
        except (OSError, TimeoutError, urllib.error.URLError, ValueError):
            delivery_failed = True
        else:
            destination_receipts = dict(receipts.get("healthchecks", {}))
            destination_receipts[state] = {
                "receipt_id": secrets.token_hex(16),
                "delivered_at": _utc(now),
            }
            receipts["healthchecks"] = destination_receipts
            _write_state(state_file, _state_payload(state, now, receipts))
            delivered = True
    if due["telegram"]:
        telegram_receipts = previous_receipts.get("telegram")
        kind = _delivery_kind(telegram_receipts, state)
        try:
            _post(
                f"{telegram_base}/bot{telegram_token}/sendMessage",
                {"chat_id": telegram_chat, "text": f"Verdaxis local {check}: {kind}"},
            )
        except (OSError, TimeoutError, urllib.error.URLError, ValueError):
            delivery_failed = True
        else:
            destination_receipts = dict(receipts.get("telegram", {}))
            destination_receipts[state] = {
                "receipt_id": secrets.token_hex(16),
                "delivered_at": _utc(now),
            }
            receipts["telegram"] = destination_receipts
            _write_state(state_file, _state_payload(state, now, receipts))
            delivered = True
    if delivery_failed:
        return "delivery_failed"
    return "delivered" if delivered else "deduped"


def _with_check_lock(check: str, state_directory: Path, operation) -> str:
    state_directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(state_directory / f".{check}.lock", os.O_CREAT | os.O_RDWR | os.O_CLOEXEC, 0o600)
    with os.fdopen(descriptor, "r+") as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        return operation()


def process_event(
    event: str,
    state_directory: Path,
    now: datetime,
    environ: Mapping[str, str],
) -> str:
    check, _state = _event_parts(event)
    return _with_check_lock(
        check,
        state_directory,
        lambda: _process_event_locked(event, state_directory, now, environ),
    )


def process_reminder(
    check: str,
    state_directory: Path,
    now: datetime,
    environ: Mapping[str, str],
) -> str:
    def remind_if_failed():
        current_state, malformed, existed = _load_state(
            state_directory / f"{check}.json", now
        )
        if malformed:
            return _process_event_locked(
                f"{check}-failure", state_directory, now, environ
            )
        if not existed:
            return "inactive"
        current = current_state.get("state")
        if current != "failure":
            return "inactive"
        return _process_event_locked(
            f"{check}-failure", state_directory, now, environ
        )

    return _with_check_lock(check, state_directory, remind_if_failed)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--event")
    action.add_argument("--remind", choices=sorted(CHECKS | {"all"}))
    parser.add_argument("--state-directory", type=Path, default=Path("/var/lib/verdaxis-monitor-alert"))
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    now = datetime.now(timezone.utc)
    if args.remind:
        checks = sorted(CHECKS) if args.remind == "all" else [args.remind]
        results = [
            process_reminder(check, args.state_directory, now, os.environ)
            for check in checks
        ]
    else:
        results = [
            process_event(
                args.event,
                args.state_directory,
                now,
                os.environ,
            )
        ]
    failed = any(result in {"delivery_failed", "delivery_unconfigured"} for result in results)
    print(f"Verdaxis monitor alert: {'failed' if failed else 'ok'}")
    return 2 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

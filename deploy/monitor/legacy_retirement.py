#!/usr/bin/env python3
"""Refuse legacy-monitor retirement unless current cutover proof is complete."""

from __future__ import annotations

import argparse
import os
import stat
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Callable


MODULE_DIRECTORY = str(Path(__file__).resolve().parent)
if MODULE_DIRECTORY not in sys.path:
    sys.path.insert(0, MODULE_DIRECTORY)

from alert_dispatch import valid_alert_state  # noqa: E402 - installed sibling
from status_state import load_json, load_monitor_status  # noqa: E402


CONFIRMATION = "RETIRE_LEGACY_AFTER_PROVEN_DUAL_RUN"
MIN_HISTORY_SECONDS = 30 * 60 * 60
MAX_OBSERVATION_GAP_SECONDS = 7 * 60
MAX_CLOCK_SKEW_SECONDS = 5 * 60
MAX_HISTORY_END_AGE_SECONDS = 15 * 60
MAX_CONFIG_BYTES = 16_384
TOP_LEVEL_KEYS = frozenset(
    {
        "schema_version",
        "first_green_at",
        "history",
        "local_alert_delivery",
        "external_monitor",
        "scheduler",
    }
)
HISTORY_KEYS = frozenset(
    {"observed_at", "local_health", "backup", "legacy_monitor"}
)
LOCAL_ALERT_KEYS = frozenset({"signed_at", "signer", "hourly_dedupe", "receipts"})
PROOF_RECEIPT_KEYS = frozenset(
    {"destination", "event", "receipt_id", "delivered_at"}
)
EXTERNAL_KEYS = frozenset({"signed_at", "signer", "coverage"})
COVERAGE_KEYS = frozenset(
    {
        "vercel_production_frontend",
        "caddy_production_api",
        "caddy_staging_api",
        "caddy_staging_frontend",
        "dns",
        "tls",
        "rendered_page",
        "alert_delivery",
        "signup_canary",
        "analytics_ingestion_canary",
    }
)
SCHEDULER_KEYS = frozenset(
    {
        "signed_at",
        "signer",
        "one_demo_scheduler_owner",
        "legacy_monitor_active_during_history",
    }
)
ALERT_CONFIG_KEYS = frozenset(
    {
        "TELEGRAM_BOT_TOKEN",
        "TELEGRAM_CHAT_ID",
        "TELEGRAM_API_BASE",
        "HEALTHCHECKS_HEALTH_URL",
        "HEALTHCHECKS_BACKUP_URL",
    }
)
ALL_RECEIPT_PAIRS = frozenset(
    {
        ("telegram", "health-failure"),
        ("telegram", "health-healthy"),
        ("telegram", "backup-failure"),
        ("telegram", "backup-healthy"),
        ("healthchecks_health", "health-failure"),
        ("healthchecks_health", "health-healthy"),
        ("healthchecks_backup", "backup-failure"),
        ("healthchecks_backup", "backup-healthy"),
    }
)
TIMER_UNITS = (
    "verdaxis-health.timer",
    "verdaxis-backup-verify.timer",
    "verdaxis-monitor-alert-reminder.timer",
    "verdaxis-demo-activity.timer",
    "verdaxis-demo-activity-staging.timer",
    "verdaxis-monitor.timer",
)


class RetirementRefused(ValueError):
    """Cutover evidence did not authorize the destructive operation."""


def _parse_utc(value: object) -> datetime:
    if not isinstance(value, str) or not value or len(value) > 40:
        raise RetirementRefused("timestamp is invalid")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        raise RetirementRefused("timestamp is invalid") from None
    if parsed.tzinfo is None:
        raise RetirementRefused("timestamp is invalid")
    return parsed.astimezone(timezone.utc)


def _validate_signer(value: object) -> None:
    if not isinstance(value, str) or not value.strip() or len(value) > 200:
        raise RetirementRefused("signer is invalid")


def _read_config_keys(path: Path) -> frozenset[str]:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise RetirementRefused("alert configuration is unsafe")
            raw = handle.read(MAX_CONFIG_BYTES + 1)
    except (OSError, RetirementRefused) as exc:
        raise RetirementRefused("alert configuration is missing or unsafe") from exc
    if len(raw) > MAX_CONFIG_BYTES:
        raise RetirementRefused("alert configuration is too large")
    try:
        lines = raw.decode("ascii").splitlines()
    except UnicodeDecodeError as exc:
        raise RetirementRefused("alert configuration is invalid") from exc
    keys: set[str] = set()
    for line in lines:
        if not line or line.lstrip().startswith("#"):
            continue
        if "=" not in line:
            raise RetirementRefused("alert configuration is invalid")
        key, _discarded = line.split("=", 1)
        if key not in ALERT_CONFIG_KEYS or key in keys:
            raise RetirementRefused("alert configuration is invalid")
        keys.add(key)
    return frozenset(keys)


def configured_receipt_matrix(path: Path) -> frozenset[tuple[str, str]]:
    keys = _read_config_keys(path)
    token = "TELEGRAM_BOT_TOKEN" in keys
    chat = "TELEGRAM_CHAT_ID" in keys
    if token != chat:
        raise RetirementRefused("Telegram configuration is incomplete")
    telegram = token and chat
    healthchecks_health = "HEALTHCHECKS_HEALTH_URL" in keys
    healthchecks_backup = "HEALTHCHECKS_BACKUP_URL" in keys
    if not (telegram or healthchecks_health) or not (
        telegram or healthchecks_backup
    ):
        raise RetirementRefused("configured alert coverage is incomplete")
    required: set[tuple[str, str]] = set()
    if telegram:
        required.update(
            {
                ("telegram", "health-failure"),
                ("telegram", "health-healthy"),
                ("telegram", "backup-failure"),
                ("telegram", "backup-healthy"),
            }
        )
    if healthchecks_health:
        required.update(
            {
                ("healthchecks_health", "health-failure"),
                ("healthchecks_health", "health-healthy"),
            }
        )
    if healthchecks_backup:
        required.update(
            {
                ("healthchecks_backup", "backup-failure"),
                ("healthchecks_backup", "backup-healthy"),
            }
        )
    return frozenset(required)


def _validate_required_receipts(
    required_receipts: frozenset[tuple[str, str]],
) -> None:
    if (
        not isinstance(required_receipts, frozenset)
        or not required_receipts
        or not required_receipts.issubset(ALL_RECEIPT_PAIRS)
    ):
        raise RetirementRefused("required alert receipt matrix is invalid")
    health_covered = any(event.startswith("health-") for _, event in required_receipts)
    backup_covered = any(event.startswith("backup-") for _, event in required_receipts)
    if not health_covered or not backup_covered:
        raise RetirementRefused("required alert receipt coverage is incomplete")


def validate_evidence(
    payload: object,
    *,
    required_receipts: frozenset[tuple[str, str]],
    now: datetime | None = None,
) -> dict:
    _validate_required_receipts(required_receipts)
    current_time = now or datetime.now(timezone.utc)
    if current_time.tzinfo is None:
        raise RetirementRefused("current time is invalid")
    current_time = current_time.astimezone(timezone.utc)
    if not isinstance(payload, dict) or set(payload) != TOP_LEVEL_KEYS:
        raise RetirementRefused("evidence schema is invalid")
    if type(payload.get("schema_version")) is not int or payload["schema_version"] != 2:
        raise RetirementRefused("evidence schema is invalid")
    first_green = _parse_utc(payload.get("first_green_at"))

    history = payload.get("history")
    if not isinstance(history, list) or not 2 <= len(history) <= 10_000:
        raise RetirementRefused("continuous history is missing")
    observed: list[datetime] = []
    for sample in history:
        if not isinstance(sample, dict) or set(sample) != HISTORY_KEYS:
            raise RetirementRefused("history sample schema is invalid")
        if any(
            sample.get(field) != "healthy"
            for field in ("local_health", "backup", "legacy_monitor")
        ):
            raise RetirementRefused("history contains a non-green sample")
        observed.append(_parse_utc(sample.get("observed_at")))
    if observed[0] != first_green:
        raise RetirementRefused("first-green timestamp does not match history")
    for earlier, later in zip(observed, observed[1:]):
        gap = (later - earlier).total_seconds()
        if gap <= 0 or gap > MAX_OBSERVATION_GAP_SECONDS:
            raise RetirementRefused("history is not continuous")
    if (observed[-1] - observed[0]).total_seconds() < MIN_HISTORY_SECONDS:
        raise RetirementRefused("dual-run history is shorter than 30 hours")
    if observed[-1] > current_time + timedelta(seconds=MAX_CLOCK_SKEW_SECONDS):
        raise RetirementRefused("observation history is in the future")
    if (current_time - observed[0]).total_seconds() < MIN_HISTORY_SECONDS:
        raise RetirementRefused("30-hour observation window has not actually elapsed")
    if (current_time - observed[-1]).total_seconds() > MAX_HISTORY_END_AGE_SECONDS:
        raise RetirementRefused("observation history did not end recently")

    local_alert = payload.get("local_alert_delivery")
    if not isinstance(local_alert, dict) or set(local_alert) != LOCAL_ALERT_KEYS:
        raise RetirementRefused("local alert signoff is invalid")
    _validate_signer(local_alert.get("signer"))
    local_signed = _parse_utc(local_alert.get("signed_at"))
    if local_alert.get("hourly_dedupe") is not True:
        raise RetirementRefused("hourly alert dedupe is not proven")
    receipts = local_alert.get("receipts")
    if not isinstance(receipts, list) or not receipts or len(receipts) > 32:
        raise RetirementRefused("local alert receipts are invalid")
    observed_pairs: set[tuple[str, str]] = set()
    receipt_ids: set[str] = set()
    for receipt in receipts:
        if not isinstance(receipt, dict) or set(receipt) != PROOF_RECEIPT_KEYS:
            raise RetirementRefused("local alert receipt schema is invalid")
        destination = receipt.get("destination")
        event = receipt.get("event")
        pair = (destination, event)
        receipt_id = receipt.get("receipt_id")
        if (
            not isinstance(destination, str)
            or not isinstance(event, str)
            or pair not in ALL_RECEIPT_PAIRS
            or pair in observed_pairs
            or not isinstance(receipt_id, str)
            or len(receipt_id) != 32
            or any(character not in "0123456789abcdef" for character in receipt_id)
            or receipt_id in receipt_ids
        ):
            raise RetirementRefused("local alert receipt is invalid")
        delivered_at = _parse_utc(receipt.get("delivered_at"))
        if (
            delivered_at < observed[0]
            or delivered_at > local_signed
            or delivered_at
            > current_time + timedelta(seconds=MAX_CLOCK_SKEW_SECONDS)
        ):
            raise RetirementRefused("local alert receipt timestamp is invalid or future")
        observed_pairs.add(pair)
        receipt_ids.add(receipt_id)
    if observed_pairs != set(required_receipts):
        raise RetirementRefused("configured local alert receipt matrix is not proven")

    external = payload.get("external_monitor")
    if not isinstance(external, dict) or set(external) != EXTERNAL_KEYS:
        raise RetirementRefused("external monitor signoff is invalid")
    _validate_signer(external.get("signer"))
    external_signed = _parse_utc(external.get("signed_at"))
    coverage = external.get("coverage")
    if (
        not isinstance(coverage, dict)
        or set(coverage) != COVERAGE_KEYS
        or any(coverage.get(field) is not True for field in COVERAGE_KEYS)
    ):
        raise RetirementRefused("external/public monitor coverage is not proven")

    scheduler = payload.get("scheduler")
    if not isinstance(scheduler, dict) or set(scheduler) != SCHEDULER_KEYS:
        raise RetirementRefused("scheduler signoff is invalid")
    _validate_signer(scheduler.get("signer"))
    scheduler_signed = _parse_utc(scheduler.get("signed_at"))
    if scheduler.get("one_demo_scheduler_owner") is not True:
        raise RetirementRefused("single scheduler ownership is not proven")
    if scheduler.get("legacy_monitor_active_during_history") is not True:
        raise RetirementRefused("legacy dual-run coverage is not proven")

    signoffs = (local_signed, external_signed, scheduler_signed)
    if any(signed < observed[-1] for signed in signoffs):
        raise RetirementRefused("signoff predates the completed observation history")
    if any(
        signed > current_time + timedelta(seconds=MAX_CLOCK_SKEW_SECONDS)
        for signed in signoffs
    ):
        raise RetirementRefused("signoff timestamp is in the future")
    return payload


def validate_evidence_file(
    path: Path,
    *,
    required_receipts: frozenset[tuple[str, str]],
    now: datetime | None = None,
) -> dict:
    loaded = load_json(path, 256 * 1024)
    if not loaded.exists or loaded.malformed:
        raise RetirementRefused("evidence file is missing or unsafe")
    return validate_evidence(
        loaded.value,
        required_receipts=required_receipts,
        now=now,
    )


def _evidence_receipts(payload: dict) -> dict[tuple[str, str], dict]:
    return {
        (receipt["destination"], receipt["event"]): {
            "receipt_id": receipt["receipt_id"],
            "delivered_at": receipt["delivered_at"],
        }
        for receipt in payload["local_alert_delivery"]["receipts"]
    }


def _validate_persisted_receipts(
    state_directory: Path,
    payload: dict,
    required_receipts: frozenset[tuple[str, str]],
    now: datetime,
) -> None:
    expected = _evidence_receipts(payload)
    for check in ("health", "backup"):
        required_for_check = {
            pair for pair in required_receipts if pair[1].startswith(f"{check}-")
        }
        loaded = load_json(state_directory / f"{check}.json", 16_384)
        if (
            not loaded.exists
            or loaded.malformed
            or not valid_alert_state(loaded.value, now)
        ):
            raise RetirementRefused("durable alert receipt state is missing or unsafe")
        state = loaded.value
        if state.get("state") != "healthy":
            raise RetirementRefused("durable alert state is not healthy")
        for destination, event in required_for_check:
            event_state = event.rsplit("-", 1)[1]
            persisted_destination = (
                "telegram" if destination == "telegram" else "healthchecks"
            )
            try:
                persisted = state["receipts"][persisted_destination][event_state]
            except (KeyError, TypeError):
                raise RetirementRefused("durable alert receipt is missing") from None
            if persisted != expected[(destination, event)]:
                raise RetirementRefused("durable alert receipt does not match evidence")
        for destination in {pair[0] for pair in required_for_check}:
            persisted_destination = (
                "telegram" if destination == "telegram" else "healthchecks"
            )
            try:
                destination_receipts = state["receipts"][persisted_destination]
                failure_at = _parse_utc(destination_receipts["failure"]["delivered_at"])
                healthy_at = _parse_utc(destination_receipts["healthy"]["delivered_at"])
            except (KeyError, TypeError):
                raise RetirementRefused(
                    "durable destination recovery receipt is missing"
                ) from None
            if healthy_at <= failure_at:
                raise RetirementRefused(
                    "destination recovery receipt is not newer than failure"
                )


def _validate_status(path: Path, name: str, now: datetime) -> None:
    payload, malformed = load_monitor_status(path)
    if malformed or not payload or payload.get("category") != "healthy":
        raise RetirementRefused(f"{name} status is not healthy")
    generated_at = _parse_utc(payload.get("generated_at"))
    current = now.astimezone(timezone.utc)
    age = (current - generated_at).total_seconds()
    if age < -MAX_CLOCK_SKEW_SECONDS or age > MAX_HISTORY_END_AGE_SECONDS:
        raise RetirementRefused(f"{name} status is stale or future")


def _validate_live_proof(
    evidence_path: Path,
    alert_config_path: Path,
    alert_state_directory: Path,
    health_status_path: Path,
    backup_status_path: Path,
    now: datetime,
) -> frozenset[tuple[str, str]]:
    required = configured_receipt_matrix(alert_config_path)
    payload = validate_evidence_file(
        evidence_path,
        required_receipts=required,
        now=now,
    )
    _validate_persisted_receipts(
        alert_state_directory,
        payload,
        required,
        now,
    )
    _validate_status(health_status_path, "health", now)
    _validate_status(backup_status_path, "backup", now)
    return required


def _run(command: list[str]) -> None:
    subprocess.run(command, check=True, close_fds=True)


def _timer_active(name: str) -> bool:
    return (
        subprocess.run(
            ["/usr/bin/systemctl", "is-active", "--quiet", name],
            check=False,
            close_fds=True,
        ).returncode
        == 0
    )


def _current_utc() -> datetime:
    return datetime.now(timezone.utc)


def guarded_retirement(
    evidence_path: Path,
    *,
    alert_config_path: Path,
    alert_state_directory: Path,
    health_status_path: Path,
    backup_status_path: Path,
    execute: bool,
    confirmation: str | None,
    runner: Callable[[list[str]], None] = _run,
    timer_checker: Callable[[str], bool] = _timer_active,
    now: datetime | None = None,
    clock: Callable[[], datetime] = _current_utc,
) -> str:
    current = now if now is not None else clock()
    if current.tzinfo is None:
        raise RetirementRefused("current time is invalid")
    current = current.astimezone(timezone.utc)
    initial_matrix = _validate_live_proof(
        evidence_path,
        alert_config_path,
        alert_state_directory,
        health_status_path,
        backup_status_path,
        current,
    )
    if not execute:
        return "validated"
    if confirmation != CONFIRMATION:
        raise RetirementRefused("exact retirement confirmation is required")
    for timer in TIMER_UNITS:
        if not timer_checker(timer):
            raise RetirementRefused(f"required timer is not active: {timer}")
    final_current = now if now is not None else clock()
    final_matrix = _validate_live_proof(
        evidence_path,
        alert_config_path,
        alert_state_directory,
        health_status_path,
        backup_status_path,
        final_current,
    )
    if final_matrix != initial_matrix:
        raise RetirementRefused("configured alert receipt matrix changed")
    for timer in TIMER_UNITS:
        if not timer_checker(timer):
            raise RetirementRefused(f"required timer is not active: {timer}")
    runner(
        [
            "/usr/bin/systemctl",
            "disable",
            "--now",
            "verdaxis-monitor.timer",
        ]
    )
    return "retired"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--evidence",
        type=Path,
        default=Path("/var/lib/verdaxis-monitor/retirement-evidence.json"),
    )
    parser.add_argument(
        "--alert-config",
        type=Path,
        default=Path("/etc/verdaxis-monitor/alert.env"),
    )
    parser.add_argument(
        "--alert-state-directory",
        type=Path,
        default=Path("/var/lib/verdaxis-monitor-alert"),
    )
    parser.add_argument(
        "--health-status",
        type=Path,
        default=Path("/var/lib/verdaxis-health/status.json"),
    )
    parser.add_argument(
        "--backup-status",
        type=Path,
        default=Path("/var/lib/verdaxis-backup-verify/status.json"),
    )
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--confirmation")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        result = guarded_retirement(
            args.evidence,
            alert_config_path=args.alert_config,
            alert_state_directory=args.alert_state_directory,
            health_status_path=args.health_status,
            backup_status_path=args.backup_status,
            execute=args.execute,
            confirmation=args.confirmation,
        )
    except (OSError, subprocess.SubprocessError, RetirementRefused):
        print("Verdaxis legacy monitor retirement: refused")
        return 2
    print(f"Verdaxis legacy monitor retirement: {result}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Read-only verification of local Verdaxis backup artifacts."""

from __future__ import annotations

import argparse
import errno
import gzip
import json
import os
import re
import stat
import sys
import time
from datetime import datetime, timezone
from pathlib import Path


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


DATABASE_PREFIXES = {
    "verdaxis": "verdaxis",
    "verdaxis_staging": "verdaxis-staging",
    "umami": "umami",
}
EXPECTED_DATABASES = frozenset(DATABASE_PREFIXES)
ARTIFACT_PATTERN = re.compile(r"(verdaxis|verdaxis-staging|umami)-(\d{8}-\d{6})\.sql\.gz")
METADATA_KEYS = frozenset({"ok", "completed_at", "backup_id", "databases"})
ATTEMPT_KEYS = frozenset(
    {"schema_version", "attempt_id", "state", "started_at", "finished_at", "backup_id"}
)
ATTEMPT_STATES = frozenset({"started", "failed", "succeeded"})
ATTEMPT_ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
MIN_UNCOMPRESSED_BYTES = 1024
SQL_MARKERS = (b"PostgreSQL database dump",)
MARKER_SCAN_BYTES = 64 * 1024


def _duration_ms(started: float) -> int:
    return max(0, round((time.monotonic() - started) * 1000))


def _result(name: str, category: str, code: str, started: float) -> dict:
    return {
        "name": name,
        "category": category,
        "code": code,
        "duration_ms": _duration_ms(started),
    }


def _metadata_failure(code: str, started: float) -> list[dict]:
    return [_result("backup_metadata", "failure", code, started)]


def _attempt_result(category: str, code: str, started: float) -> dict:
    return _result("backup_attempt", category, code, started)


def _parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str) or not value or len(value) > 40:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed.astimezone(timezone.utc) if parsed.tzinfo is not None else None


def _read_attempt_record(
    directory_descriptor: int,
    name: str,
    max_bytes: int,
) -> object | None:
    try:
        descriptor = os.open(
            name,
            os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
            dir_fd=directory_descriptor,
        )
        with os.fdopen(descriptor, "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                return None
            raw = handle.read(max_bytes + 1)
    except OSError:
        return None
    if len(raw) > max_bytes:
        return None
    try:
        return json.loads(raw, object_pairs_hook=reject_duplicate_keys)
    except (UnicodeDecodeError, ValueError, RecursionError):
        return None


def _validate_attempt_record(
    payload: object,
    filename: str,
    now: datetime,
    future_tolerance_seconds: int,
) -> tuple[datetime, str, datetime | None, str | None] | None:
    if not isinstance(payload, dict) or set(payload) != ATTEMPT_KEYS:
        return None
    if type(payload.get("schema_version")) is not int or payload["schema_version"] != 1:
        return None
    attempt_id = payload.get("attempt_id")
    state = payload.get("state")
    if (
        not isinstance(attempt_id, str)
        or ATTEMPT_ID_PATTERN.fullmatch(attempt_id) is None
        or filename != f"{attempt_id}.json"
        or not isinstance(state, str)
        or state not in ATTEMPT_STATES
    ):
        return None
    started_at = _parse_timestamp(payload.get("started_at"))
    if started_at is None:
        return None
    current = now.astimezone(timezone.utc)
    if (started_at - current).total_seconds() > future_tolerance_seconds:
        return None
    finished_raw = payload.get("finished_at")
    backup_id = payload.get("backup_id")
    if state == "started":
        if finished_raw is not None or backup_id is not None:
            return None
        return started_at, state, None, None
    finished_at = _parse_timestamp(finished_raw)
    if (
        finished_at is None
        or finished_at < started_at
        or (finished_at - current).total_seconds() > future_tolerance_seconds
    ):
        return None
    if state == "failed":
        if backup_id is not None:
            return None
        return started_at, state, finished_at, None
    if (
        not isinstance(backup_id, str)
        or re.fullmatch(r"\d{8}-\d{6}", backup_id) is None
    ):
        return None
    return started_at, state, finished_at, backup_id


def _latest_attempt(
    directory: Path,
    *,
    now: datetime,
    future_tolerance_seconds: int,
    max_records: int,
    max_record_bytes: int,
    deadline: float,
) -> tuple[tuple[datetime, str, datetime | None, str | None] | None, str | None]:
    try:
        directory_descriptor = os.open(
            directory,
            os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW,
        )
    except FileNotFoundError:
        return None, "attempt_evidence_missing"
    except OSError:
        return None, "attempt_evidence_invalid"
    records: list[tuple[datetime, str, datetime | None, str | None]] = []
    try:
        with os.scandir(directory_descriptor) as entries:
            for count, entry in enumerate(entries, start=1):
                if count > max_records:
                    return None, "attempt_evidence_invalid"
                if time.monotonic() >= deadline:
                    return None, "total_deadline_exceeded"
                if not entry.name.endswith(".json"):
                    continue
                if not entry.is_file(follow_symlinks=False):
                    return None, "attempt_evidence_invalid"
                payload = _read_attempt_record(
                    directory_descriptor, entry.name, max_record_bytes
                )
                record = _validate_attempt_record(
                    payload,
                    entry.name,
                    now,
                    future_tolerance_seconds,
                )
                if record is None:
                    return None, "attempt_evidence_invalid"
                records.append(record)
    except OSError:
        return None, "attempt_evidence_invalid"
    finally:
        os.close(directory_descriptor)
    if not records:
        return None, "attempt_evidence_missing"
    started_times = [record[0] for record in records]
    if len(set(started_times)) != len(started_times):
        return None, "attempt_evidence_invalid"
    return max(records, key=lambda record: record[0]), None


def _read_metadata(path: Path, max_bytes: int, started: float) -> tuple[dict | None, list[dict]]:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                return None, _metadata_failure("metadata_invalid_type", started)
            raw = handle.read(max_bytes + 1)
    except FileNotFoundError:
        return None, _metadata_failure("metadata_missing", started)
    except OSError as exc:
        if exc.errno == errno.ELOOP:
            return None, _metadata_failure("metadata_invalid_type", started)
        return None, _metadata_failure("metadata_unreadable", started)
    if len(raw) > max_bytes:
        return None, _metadata_failure("metadata_too_large", started)
    try:
        payload = json.loads(raw, object_pairs_hook=reject_duplicate_keys)
    except (UnicodeDecodeError, ValueError, RecursionError):
        return None, _metadata_failure("metadata_invalid_json", started)
    if not isinstance(payload, dict):
        return None, _metadata_failure("metadata_invalid_json", started)
    return payload, []


def _validate_metadata(
    payload: dict,
    now: datetime,
    max_age_seconds: int,
    future_tolerance_seconds: int,
    max_timestamp_skew_seconds: int,
    started: float,
) -> tuple[tuple[str, datetime] | None, list[dict]]:
    if set(payload) != METADATA_KEYS:
        return None, _metadata_failure("metadata_schema_mismatch", started)
    if payload.get("ok") is not True:
        return None, _metadata_failure("backup_incomplete", started)
    inventory = payload.get("databases")
    if (
        not isinstance(inventory, list)
        or len(inventory) != len(EXPECTED_DATABASES)
        or any(not isinstance(item, str) for item in inventory)
        or set(inventory) != EXPECTED_DATABASES
    ):
        return None, _metadata_failure("unexpected_inventory", started)
    backup_id = payload.get("backup_id")
    if not isinstance(backup_id, str) or re.fullmatch(r"\d{8}-\d{6}", backup_id) is None:
        return None, _metadata_failure("invalid_backup_id", started)
    completed_raw = payload.get("completed_at")
    if not isinstance(completed_raw, str):
        return None, _metadata_failure("invalid_timestamp", started)
    try:
        completed_at = datetime.fromisoformat(completed_raw.replace("Z", "+00:00"))
        if completed_at.tzinfo is None:
            raise ValueError
    except ValueError:
        return None, _metadata_failure("invalid_timestamp", started)
    age = (now.astimezone(timezone.utc) - completed_at.astimezone(timezone.utc)).total_seconds()
    if age < -future_tolerance_seconds:
        return None, _metadata_failure("future_timestamp", started)
    if age > max_age_seconds:
        return None, _metadata_failure("stale_status", started)
    try:
        backup_time = datetime.strptime(backup_id, "%Y%m%d-%H%M%S").replace(tzinfo=timezone.utc)
    except ValueError:
        return None, _metadata_failure("invalid_backup_id", started)
    if abs((completed_at.astimezone(timezone.utc) - backup_time).total_seconds()) > max_timestamp_skew_seconds:
        return None, _metadata_failure("backup_id_timestamp_mismatch", started)
    return (backup_id, completed_at.astimezone(timezone.utc)), []


def _newest_artifacts(
    directory: Path,
    max_entries: int,
    deadline: float,
) -> tuple[dict[str, str], str | None]:
    newest: dict[str, str] = {}
    prefix_to_database = {prefix: database for database, prefix in DATABASE_PREFIXES.items()}
    try:
        with os.scandir(directory) as entries:
            for count, entry in enumerate(entries, start=1):
                if count > max_entries:
                    return {}, "artifact_entry_limit"
                if time.monotonic() >= deadline:
                    return {}, "total_deadline_exceeded"
                if not entry.is_file(follow_symlinks=False):
                    continue
                match = ARTIFACT_PATTERN.fullmatch(entry.name)
                if not match:
                    continue
                database = prefix_to_database[match.group(1)]
                newest[database] = max(newest.get(database, ""), match.group(2))
    except OSError:
        return {}, "backup_directory_unreadable"
    return newest, None


def _check_gzip(
    name: str,
    path: Path,
    max_artifact_bytes: int,
    max_uncompressed_bytes: int,
    completed_at: datetime,
    max_timestamp_skew_seconds: int,
    deadline: float,
) -> dict:
    started = time.monotonic()
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except FileNotFoundError:
        return _result(name, "failure", "artifact_missing", started)
    except OSError as exc:
        if exc.errno == errno.ELOOP:
            return _result(name, "failure", "artifact_invalid_type", started)
        return _result(name, "failure", "artifact_unreadable", started)
    with os.fdopen(descriptor, "rb") as raw:
        file_status = os.fstat(raw.fileno())
        if not stat.S_ISREG(file_status.st_mode):
            return _result(name, "failure", "artifact_invalid_type", started)
        if file_status.st_size == 0:
            return _result(name, "failure", "artifact_empty", started)
        if file_status.st_size > max_artifact_bytes:
            return _result(name, "failure", "artifact_too_large", started)
        artifact_time = datetime.fromtimestamp(file_status.st_mtime, tz=timezone.utc)
        if abs((completed_at - artifact_time).total_seconds()) > max_timestamp_skew_seconds:
            return _result(name, "failure", "artifact_timestamp_mismatch", started)
        expanded = 0
        prefix = bytearray()
        try:
            with gzip.GzipFile(fileobj=raw, mode="rb") as handle:
                while True:
                    if time.monotonic() >= deadline:
                        return _result(name, "failure", "total_deadline_exceeded", started)
                    chunk = handle.read(1024 * 1024)
                    if not chunk:
                        break
                    expanded += len(chunk)
                    if len(prefix) < MARKER_SCAN_BYTES:
                        prefix.extend(chunk[: MARKER_SCAN_BYTES - len(prefix)])
                    if expanded > max_uncompressed_bytes:
                        return _result(name, "failure", "uncompressed_too_large", started)
        except (gzip.BadGzipFile, EOFError, OSError):
            return _result(name, "failure", "gzip_invalid", started)
        if expanded < MIN_UNCOMPRESSED_BYTES or not any(
            marker in prefix for marker in SQL_MARKERS
        ):
            return _result(name, "failure", "gzip_implausible", started)
    return _result(name, "healthy", "ok", started)


def verify_backups(
    status_file: Path,
    backup_directory: Path,
    attempt_directory: Path,
    *,
    now: datetime,
    max_age_seconds: int,
    future_tolerance_seconds: int,
    max_timestamp_skew_seconds: int,
    max_metadata_bytes: int,
    max_attempt_records: int,
    max_attempt_bytes: int,
    max_artifacts: int,
    max_artifact_bytes: int,
    max_uncompressed_bytes: int,
    total_timeout_seconds: float,
) -> list[dict]:
    started = time.monotonic()
    deadline = started + total_timeout_seconds
    latest_attempt, attempt_error = _latest_attempt(
        attempt_directory,
        now=now,
        future_tolerance_seconds=future_tolerance_seconds,
        max_records=max_attempt_records,
        max_record_bytes=max_attempt_bytes,
        deadline=deadline,
    )
    if attempt_error:
        return [_attempt_result("failure", attempt_error, started)]
    _attempt_started, attempt_state, attempt_finished, attempt_backup_id = latest_attempt
    if attempt_state == "started":
        return [_attempt_result("failure", "backup_attempt_incomplete", started)]
    if attempt_state == "failed":
        return [_attempt_result("failure", "backup_attempt_failed", started)]
    payload, failures = _read_metadata(status_file, max_metadata_bytes, started)
    if failures:
        return failures
    metadata_identity, failures = _validate_metadata(
        payload,
        now,
        max_age_seconds,
        future_tolerance_seconds,
        max_timestamp_skew_seconds,
        started,
    )
    if failures:
        return failures
    backup_id, completed_at = metadata_identity
    if (
        attempt_backup_id != backup_id
        or attempt_finished is None
        or completed_at < _attempt_started
        or abs((completed_at - attempt_finished).total_seconds())
        > max_timestamp_skew_seconds
    ):
        return [_attempt_result("failure", "attempt_status_mismatch", started)]
    attempt = _attempt_result("healthy", "ok", started)
    newest, discovery_error = _newest_artifacts(backup_directory, max_artifacts, deadline)
    metadata_code = discovery_error or "ok"
    if not discovery_error and any(value != backup_id for value in newest.values()):
        metadata_code = "status_not_newest"
    metadata = _result(
        "backup_metadata",
        "healthy" if metadata_code == "ok" else "failure",
        metadata_code,
        started,
    )
    results = [attempt, metadata]
    for database, prefix in DATABASE_PREFIXES.items():
        results.append(
            _check_gzip(
                f"{database}_artifact",
                backup_directory / f"{prefix}-{backup_id}.sql.gz",
                max_artifact_bytes,
                max_uncompressed_bytes,
                completed_at,
                max_timestamp_skew_seconds,
                deadline,
            )
        )
    return results


def run_monitor(args: argparse.Namespace) -> dict:
    output = Path(args.output)
    generated_at = datetime.now(timezone.utc)
    previous, malformed = load_monitor_status(output)
    if malformed:
        quarantine(output)
        return failure_status(
            "previous_status_invalid", generated_at, name="backup_monitor"
        )
    checks = verify_backups(
        Path(args.backup_status),
        Path(args.backup_directory),
        Path(args.attempt_directory),
        now=generated_at,
        max_age_seconds=args.max_age,
        future_tolerance_seconds=args.future_tolerance,
        max_timestamp_skew_seconds=args.max_timestamp_skew,
        max_metadata_bytes=args.max_metadata_bytes,
        max_attempt_records=args.max_attempt_records,
        max_attempt_bytes=args.max_attempt_bytes,
        max_artifacts=args.max_artifacts,
        max_artifact_bytes=args.max_artifact_bytes,
        max_uncompressed_bytes=args.max_uncompressed_bytes,
        total_timeout_seconds=args.total_timeout,
    )
    return build_status(checks, previous, generated_at)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backup-status", default="/home/verdaxis-prod/backups/status.json")
    parser.add_argument("--backup-directory", default="/home/verdaxis-prod/backups")
    parser.add_argument(
        "--attempt-directory", default="/home/verdaxis-prod/backups/attempts"
    )
    parser.add_argument("--output", default="/var/lib/verdaxis-backup-verify/status.json")
    parser.add_argument("--max-age", type=int, default=30 * 60 * 60)
    parser.add_argument("--future-tolerance", type=int, default=300)
    parser.add_argument("--max-timestamp-skew", type=int, default=5)
    parser.add_argument("--max-metadata-bytes", type=int, default=16_384)
    parser.add_argument("--max-attempt-records", type=int, default=1_000)
    parser.add_argument("--max-attempt-bytes", type=int, default=4_096)
    parser.add_argument("--max-artifacts", type=int, default=1_000)
    parser.add_argument("--max-artifact-bytes", type=int, default=20 * 1024**3)
    parser.add_argument("--max-uncompressed-bytes", type=int, default=100 * 1024**3)
    parser.add_argument("--total-timeout", type=float, default=120.0)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        payload = run_monitor(args)
    except Exception:
        payload = failure_status(
            "fatal_error", datetime.now(timezone.utc), name="backup_monitor"
        )
    atomic_write_status(Path(args.output), payload)
    print(f"Verdaxis backup verification: {payload['category']}")
    return 0 if payload["category"] == "healthy" else 2


if __name__ == "__main__":
    raise SystemExit(main())

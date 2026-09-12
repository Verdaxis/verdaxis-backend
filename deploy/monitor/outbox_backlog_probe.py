#!/usr/bin/env python3
"""Read-only probe for the market event outbox sequencing backlog.

A wedged-but-alive sequencer leader (holds the advisory lock but stops
assigning) grows an unsequenced backlog with no other signal: workers stay
healthy, SSE connections stay open, and nothing pages. This probe exposes
the two numbers that make that failure visible:

- ``pending_count``: rows with ``stream_seq IS NULL``
- ``oldest_pending_seconds``: age of the oldest unsequenced row

It runs one read-only SELECT through ``psql`` (peer/.pgpass auth; never a
password argument), prints a single JSON object, and exits 0 (ok),
1 (threshold breached), or 2 (probe error). The canonical five-minute monitor
invokes the installed probe for production and staging.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from datetime import datetime, timezone

BACKLOG_QUERY = (
    "SELECT count(*),"
    " COALESCE(EXTRACT(EPOCH FROM (now() - min(created_at))), 0)::bigint"
    " FROM public.market_event_outbox WHERE stream_seq IS NULL"
)


class ProbeError(RuntimeError):
    """The probe could not obtain a trustworthy backlog measurement."""


def run_backlog_query(dsn: str, timeout: int) -> str:
    psql = shutil.which("psql")
    if psql is None:
        raise ProbeError("psql is not available")
    try:
        completed = subprocess.run(
            [
                psql,
                "--no-psqlrc",
                "--tuples-only",
                "--no-align",
                "--field-separator=|",
                "--set=ON_ERROR_STOP=1",
                "--dbname",
                dsn,
                "--command",
                BACKLOG_QUERY,
            ],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise ProbeError("backlog query timed out") from exc
    if completed.returncode != 0:
        raise ProbeError("psql query failed")
    return completed.stdout


def parse_backlog(output: str) -> tuple[int, int]:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if len(lines) != 1:
        raise ProbeError("expected exactly one result row")
    parts = lines[0].split("|")
    if len(parts) != 2:
        raise ProbeError("expected exactly two result columns")
    try:
        pending_count, oldest_seconds = int(parts[0]), int(parts[1])
    except ValueError as exc:
        raise ProbeError("non-integer backlog measurement") from exc
    if pending_count < 0 or oldest_seconds < 0:
        raise ProbeError("negative backlog measurement")
    return pending_count, oldest_seconds


def evaluate(
    pending_count: int,
    oldest_seconds: int,
    *,
    max_pending: int,
    max_age_seconds: int,
) -> dict:
    breaches = []
    if pending_count > max_pending:
        breaches.append("pending_count")
    if oldest_seconds > max_age_seconds:
        breaches.append("oldest_pending_seconds")
    return {
        "probe": "outbox_backlog",
        "ok": not breaches,
        "breaches": breaches,
        "pending_count": pending_count,
        "oldest_pending_seconds": oldest_seconds,
        "max_pending": max_pending,
        "max_age_seconds": max_age_seconds,
        "checked_at": datetime.now(timezone.utc).isoformat(),
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dsn",
        required=True,
        help="read-only libpq DSN/URI, e.g. 'dbname=verdaxis user=verdaxis_backup'",
    )
    parser.add_argument("--max-pending", type=int, default=1000)
    parser.add_argument(
        "--max-age-seconds",
        type=int,
        default=300,
        help="sequencer normally assigns within ~2s; sustained age means a wedged leader",
    )
    parser.add_argument("--query-timeout", type=int, default=20)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None, query_runner=run_backlog_query) -> int:
    args = parse_args(argv)
    try:
        pending_count, oldest_seconds = parse_backlog(
            query_runner(args.dsn, args.query_timeout)
        )
    except ProbeError as exc:
        print(json.dumps({"probe": "outbox_backlog", "ok": False, "error": str(exc)}))
        return 2
    status = evaluate(
        pending_count,
        oldest_seconds,
        max_pending=args.max_pending,
        max_age_seconds=args.max_age_seconds,
    )
    print(json.dumps(status, sort_keys=True))
    return 0 if status["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())

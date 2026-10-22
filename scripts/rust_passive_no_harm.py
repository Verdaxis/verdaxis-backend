#!/usr/bin/env python3
"""Evaluate equal passive access/journal windows; never make network requests."""
from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import re
import sys
from functools import lru_cache
from pathlib import Path
from urllib.parse import urlsplit

# Support the documented direct-file CLI as well as module/test imports.
if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.rust_caddy_candidate import GROUPS, production_group


@lru_cache(maxsize=4)
def matcher_block(group: str) -> str:
    return production_group(group)


def matches(group: str, method: str, uri: str) -> bool:
    if method != "GET" or group not in GROUPS[:-1]:
        return False
    path = urlsplit(uri).path
    block = matcher_block(group)
    for line in block.splitlines():
        fields = line.strip().split()
        if fields[:1] == ["path"]:
            return path in fields[1:]
        if fields[:1] == ["path_regexp"]:
            return re.fullmatch(fields[2], path) is not None
    raise ValueError("group has no reviewed matcher")


def p95(values: list[float]) -> float:
    return sorted(values)[math.ceil(0.95 * len(values)) - 1]


def timestamp(value) -> float:
    if isinstance(value, str):
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            raise ValueError("timestamp requires an explicit timezone")
        value = parsed.timestamp()
    if not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError("invalid log timestamp")
    return float(value)


def evaluate(rows, group, cutover, seconds, ready, before_errors, after_errors):
    if group not in GROUPS[:-1] or seconds <= 0:
        raise ValueError("invalid group or passive window length")
    cutover = timestamp(cutover)
    windows = [[], []]
    failures = [0, 0]
    for row in rows:
        instant = timestamp(row.get("ts"))
        if not cutover - seconds <= instant < cutover + seconds:
            continue
        request = row.get("request", {})
        if not matches(group, request.get("method"), request.get("uri", "")):
            continue
        status, duration = row.get("status"), row.get("duration")
        if (not isinstance(status, int) or not 100 <= status <= 599
                or not isinstance(duration, (int, float))
                or not math.isfinite(duration) or duration < 0):
            raise ValueError("selected access row lacks valid status/duration")
        index = int(instant >= cutover)
        windows[index].append(duration)
        failures[index] += int(status >= 500)
    counts = [len(window) for window in windows]
    reasons = []
    if not ready:
        reasons.append("Rust readiness failed")
    if failures[0] == 0 and failures[1] >= 1:
        reasons.append("new group 5xx")
    if all(counts) and 200 * (failures[1] * counts[0] - failures[0] * counts[1]) > counts[0] * counts[1]:
        reasons.append("group 5xx rate increased by more than 0.5 percentage points")
    enough = all(count >= 50 for count in counts)
    if enough and p95(windows[1]) > 2 * p95(windows[0]):
        reasons.append("group p95 exceeded twice the before value")
    if after_errors - before_errors:
        reasons.append("new Python/Rust error class")
    return {
        "decision": "ROLLBACK" if reasons else ("PASS" if enough else "HOLD"),
        "reasons": reasons,
        "group": group, "seconds_per_window": seconds,
        "requests": counts, "errors_5xx": failures,
        "p95_seconds": [p95(window) if window else None for window in windows],
        "new_error_classes": sorted(after_errors - before_errors),
        "note": "fewer than 50 requests in either window is inconclusive" if not enough else "passive existing traffic only",
    }


def error_classes(rows) -> set[str]:
    result = set()
    for row in rows:
        message = row.get("MESSAGE", "")
        try:
            body = json.loads(message)
        except (ValueError, TypeError):
            body = {}
        if not isinstance(body, dict):
            body = {}
        level = str(body.get("level", body.get("levelname", ""))).lower()
        if level not in {"error", "critical", "fatal"} and not re.search(r"\b(ERROR|CRITICAL|Traceback|[A-Za-z]+Error:)", message):
            continue
        kind = body.get("error_type") or body.get("exception_type")
        if not isinstance(kind, str):
            found = re.search(r"\b([A-Za-z_][A-Za-z0-9_.]*(?:Error|Exception))\b", message)
            # An unclassified error must not be silently ignored. Retain a
            # private event fingerprint without publishing message contents.
            kind = found[1] if found else str(body.get("event", message))
        result.add(f"{row.get('_SYSTEMD_UNIT', 'unknown-unit')}:{kind}")
    return result


class InvalidEvidence(ValueError):
    """The evidence cannot support a decision; the message is safe to print."""


def journal_rows(path: Path, label: str) -> list[dict]:
    """Read one extracted journal window; an empty or missing file is never "no errors"."""
    try:
        rows = list(json_lines([path]))
    except OSError as exc:
        raise InvalidEvidence(f"{label} journal cannot be read") from exc
    if not rows:
        raise InvalidEvidence(f"{label} journal is empty")
    return rows


def json_lines(paths):
    for path in paths:
        with path.open() as stream:
            for line in stream:
                if len(line) > 4 * 1024 * 1024:
                    raise ValueError("oversized log row")
                if line.strip():
                    yield json.loads(line)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--access-log", type=Path, action="append", required=True)
    parser.add_argument("--before-journal", type=Path, required=True)
    parser.add_argument("--after-journal", type=Path, required=True)
    parser.add_argument("--cutover", required=True, help="UTC ISO timestamp of successful reload")
    parser.add_argument("--window-seconds", type=int, default=1800)
    parser.add_argument("--group", choices=GROUPS[:-1], required=True)
    parser.add_argument("--readiness", type=Path, required=True)
    parser.add_argument("--environment", choices=("production", "staging"), required=True)
    parser.add_argument("--release-sha", required=True)
    args = parser.parse_args()
    try:
        if not re.fullmatch(r"[0-9a-f]{40}", args.release_sha):
            raise ValueError("full release SHA required")
        ready = json.loads(args.readiness.read_text()) == {
            "status": "ok", "db": "ok", "environment": args.environment,
            "release_sha": args.release_sha,
        }
        report = evaluate(json_lines(args.access_log), args.group, timestamp(args.cutover),
                          args.window_seconds, ready,
                          error_classes(journal_rows(args.before_journal, "before")),
                          error_classes(journal_rows(args.after_journal, "after")))
    except InvalidEvidence as exc:
        print(f"Passive gate refused: {exc}; review private inputs", file=sys.stderr)
        return 2
    except (ValueError, OSError, TypeError) as exc:
        print(f"Passive gate refused: {type(exc).__name__}; review private inputs", file=sys.stderr)
        return 2
    print(json.dumps(report, sort_keys=True))
    return {"PASS": 0, "ROLLBACK": 1, "HOLD": 2}[report["decision"]]


if __name__ == "__main__":
    raise SystemExit(main())

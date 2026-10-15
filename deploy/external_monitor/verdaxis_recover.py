#!/usr/bin/env python3
"""Attempt one bounded Verdaxis recovery before read-only diagnosis."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


DEFAULT_STATUS_FILE = "/var/lib/verdaxis-monitor/status.json"
DEFAULT_STATE_DIR = "/var/lib/verdaxis-recovery"
DEFAULT_COOLDOWN_SECONDS = 3600
MAX_SOURCE_BYTES = 256 * 1024
MAX_OUTPUT = 2_000
MIN_BACKUP_FREE_BYTES = 10 * 1024 * 1024 * 1024

API_TARGETS = {
    "production_api": {
        "endpoint": "verdaxis api",
        "environment": "production",
        "unit": "verdaxis-backend.service",
        "local_url": "http://127.0.0.1:8000/health/ready",
        "public_url": "https://api.verdaxis.exchange/health/ready",
    },
    "staging_api": {
        "endpoint": "verdaxis staging api",
        "environment": "staging",
        "unit": "verdaxis-backend-staging.service",
        "local_url": "http://127.0.0.1:8001/health/ready",
        "public_url": "https://api-staging.verdaxis.exchange/health/ready",
    },
}
DEPLOY_STATE_FILES = (
    Path(
        "/home/verdaxis-prod/verdaxis/prod/be/"
        ".runtime-deploy/production.state"
    ),
    Path(
        "/home/verdaxis-prod/verdaxis/staging/be/"
        ".runtime-deploy/staging.state"
    ),
)
BACKUP_ERROR_PREFIXES = (
    "backup status is missing:",
    "backup status is unreadable:",
    "last database backup did not complete successfully",
    "last database backup did not include",
    "database backup is stale ",
    "backup artifact is empty:",
    "backup artifact is missing:",
    "backup artifact failed gzip validation:",
)
SECRET_RE = re.compile(
    r"(?i)\b(token|secret|password|authorization|api[_-]?key)"
    r"(\s*[:=]\s*)([^\s,;]+)"
)
LONG_VALUE_RE = re.compile(r"\b[A-Za-z0-9_+/=-]{80,}\b")


def log(message: str) -> None:
    print(f"verdaxis-recovery: {message}", file=sys.stderr)


def redact(value: str, limit: int = MAX_OUTPUT) -> str:
    value = SECRET_RE.sub(r"\1\2[redacted]", value)
    value = LONG_VALUE_RE.sub("[redacted-long-value]", value)
    if len(value) > limit:
        return value[-limit:] + "\n[output truncated]"
    return value


def read_json(path: Path) -> dict[str, Any]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"{path} is not a regular file")
        if metadata.st_size > MAX_SOURCE_BYTES:
            raise ValueError(f"{path} exceeds {MAX_SOURCE_BYTES} bytes")
        with os.fdopen(fd, encoding="utf-8") as handle:
            fd = -1
            payload = json.load(handle)
    finally:
        if fd >= 0:
            os.close(fd)
    if not isinstance(payload, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return payload


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.chmod(0o640)
    temporary.replace(path)


def failure_fingerprint(status: dict[str, Any]) -> str:
    errors = sorted(
        error for error in status.get("errors", []) if isinstance(error, str)
    )
    failed_endpoints = sorted(
        str(endpoint.get("name", ""))
        for endpoint in status.get("endpoints", [])
        if isinstance(endpoint, dict) and endpoint.get("ok") is not True
    )
    basis = json.dumps(
        {"errors": errors, "failed_endpoints": failed_endpoints},
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(basis.encode()).hexdigest()


def requested_targets(status: dict[str, Any]) -> list[str]:
    failed = {
        endpoint.get("name")
        for endpoint in status.get("endpoints", [])
        if isinstance(endpoint, dict) and endpoint.get("ok") is not True
    }
    targets = [
        target
        for target, config in API_TARGETS.items()
        if config["endpoint"] in failed
    ]
    errors = [
        error for error in status.get("errors", []) if isinstance(error, str)
    ]
    if any(error.startswith(BACKUP_ERROR_PREFIXES) for error in errors):
        targets.append("backup")
    return targets


def caddy_monitor_clean(status: dict[str, Any]) -> bool:
    prefixes = (
        "/etc/caddy/",
        "caddy validate failed:",
        "caddy adapt failed:",
        "caddy adapt returned invalid json:",
        "missing caddy host blocks:",
    )
    return not any(
        isinstance(error, str) and error.lower().startswith(prefixes)
        for error in status.get("errors", [])
    )


def run_command(
    command: list[str], timeout: int = 120
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            command,
            capture_output=True,
            check=False,
            text=True,
            timeout=timeout,
            env={"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8"},
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return subprocess.CompletedProcess(command, 124, "", str(exc))


def command_action(
    report: dict[str, Any],
    name: str,
    command: list[str],
    timeout: int = 120,
) -> subprocess.CompletedProcess[str]:
    result = run_command(command, timeout)
    output = "\n".join(
        part.strip() for part in (result.stdout, result.stderr) if part.strip()
    )
    report["actions"].append(
        {
            "name": name,
            "command": command,
            "returncode": result.returncode,
            "output": redact(output),
        }
    )
    return result


def record_event(
    report: dict[str, Any], name: str, outcome: str, detail: str
) -> None:
    report["actions"].append(
        {"name": name, "outcome": outcome, "detail": redact(detail)}
    )


def probe_ready(url: str, environment: str) -> str:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "Verdaxis-Recovery/1.0"},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=8) as response:
            body = response.read(32 * 1024).decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        body = exc.read(32 * 1024).decode("utf-8", errors="replace")
    except (OSError, urllib.error.URLError, TimeoutError):
        return "unavailable"
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return "unavailable"
    if not isinstance(payload, dict):
        return "unavailable"
    if "db" in payload and payload.get("db") != "ok":
        return "database_unhealthy"
    release_sha = payload.get("release_sha")
    if (
        payload.get("status") == "ok"
        and payload.get("environment") == environment
        and isinstance(release_sha, str)
        and re.fullmatch(r"[0-9a-f]{40}", release_sha)
    ):
        return "healthy"
    return "unavailable"


def probe_local(environment: str) -> str:
    config = next(
        item
        for item in API_TARGETS.values()
        if item["environment"] == environment
    )
    return probe_ready(config["local_url"], environment)


def probe_public(environment: str) -> str:
    config = next(
        item
        for item in API_TARGETS.values()
        if item["environment"] == environment
    )
    return probe_ready(config["public_url"], environment)


def wait_for_local(environment: str) -> bool:
    for _ in range(24):
        if probe_local(environment) == "healthy":
            return True
        time.sleep(5)
    return False


def deployment_active() -> bool:
    return any(path.exists() for path in DEPLOY_STATE_FILES)


def backup_has_headroom() -> bool:
    usage = shutil.disk_usage("/")
    return usage.free >= MIN_BACKUP_FREE_BYTES and (
        usage.free / usage.total if usage.total else 0
    ) >= 0.05


def recover_api(
    target: str, report: dict[str, Any]
) -> tuple[bool, bool]:
    config = API_TARGETS[target]
    environment = config["environment"]
    local_state = probe_local(environment)
    attempted = False

    if local_state == "database_unhealthy":
        record_event(
            report,
            f"{target}_restart",
            "refused",
            "database readiness is unhealthy; restarting the API is not a "
            "database recovery",
        )
        return False, False

    if local_state != "healthy":
        if deployment_active():
            record_event(
                report,
                f"{target}_restart",
                "refused",
                "a deployment transaction became active",
            )
            return False, False
        command_action(
            report,
            f"{target}_reset_failed",
            ["/bin/systemctl", "reset-failed", config["unit"]],
        )
        restart = command_action(
            report,
            f"{target}_restart",
            ["/bin/systemctl", "restart", config["unit"]],
        )
        attempted = True
        if restart.returncode != 0 or not wait_for_local(environment):
            record_event(
                report,
                f"{target}_readiness",
                "failed",
                "loopback readiness did not recover after restart",
            )
            return attempted, False

    if probe_public(environment) == "healthy":
        record_event(
            report,
            f"{target}_public_readiness",
            "recovered",
            "public readiness is healthy",
        )
        return attempted, False
    return attempted, True


def recover_caddy(report: dict[str, Any], status: dict[str, Any]) -> bool:
    if deployment_active() or not caddy_monitor_clean(status):
        record_event(
            report,
            "caddy",
            "refused",
            "deployment state or monitor Caddy validation forbids recovery",
        )
        return False
    active = run_command(
        ["/bin/systemctl", "is-active", "--quiet", "caddy.service"]
    )
    operation = "reload" if active.returncode == 0 else "restart"
    result = command_action(
        report,
        f"caddy_{operation}",
        ["/bin/systemctl", operation, "caddy.service"],
    )
    return result.returncode == 0


def recover_backup(report: dict[str, Any]) -> bool:
    if deployment_active() or not backup_has_headroom():
        record_event(
            report,
            "backup",
            "refused",
            "deployment state or backup filesystem headroom forbids recovery",
        )
        return False
    result = command_action(
        report,
        "backup",
        ["/bin/systemctl", "start", "verdaxis-backup.service"],
        timeout=900,
    )
    return result.returncode == 0


def run_full_monitor() -> int:
    result = run_command(
        [
            "/bin/systemctl",
            "start",
            "verdaxis-monitor-verify.service",
        ],
        timeout=600,
    )
    return result.returncode


def telegram_send(message: str) -> None:
    token = os.getenv("TELEGRAM_BOT_TOKEN", "").strip()
    chat_id = os.getenv("TELEGRAM_CHAT_ID", "").strip()
    if not token or not chat_id:
        return
    data = urllib.parse.urlencode(
        {"chat_id": chat_id, "text": message[:3900]}
    ).encode()
    request = urllib.request.Request(
        f"https://api.telegram.org/bot{token}/sendMessage",
        data=data,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            response.read()
    except Exception as exc:
        detail = redact(str(exc).replace(token, "[redacted]"), 500)
        log(f"Telegram delivery failed: {detail}")


def prune_reports(state_dir: Path, keep: int = 40) -> None:
    reports = sorted(state_dir.glob("recovery-*.json"))
    for path in reports[:-keep]:
        path.unlink(missing_ok=True)


def main() -> int:
    status_file = Path(
        os.getenv("RECOVERY_STATUS_FILE", DEFAULT_STATUS_FILE)
    )
    state_dir = Path(os.getenv("RECOVERY_STATE_DIR", DEFAULT_STATE_DIR))
    state_dir.mkdir(parents=True, exist_ok=True)
    with (state_dir / "recover.lock").open("a+", encoding="utf-8") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            log("another recovery is already running")
            return 2

        try:
            status = read_json(status_file)
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            log(f"monitor status is unavailable: {redact(str(exc))}")
            return 2
        if status.get("ok") is True:
            log("monitor status is healthy; no recovery needed")
            return 0

        fingerprint = failure_fingerprint(status)
        state_file = state_dir / "state.json"
        try:
            state = read_json(state_file)
        except (OSError, ValueError, json.JSONDecodeError):
            state = {}
        now = int(time.time())
        cooldown = int(
            os.getenv(
                "VERDAXIS_RECOVERY_COOLDOWN_SECONDS",
                str(DEFAULT_COOLDOWN_SECONDS),
            )
        )
        if (
            state.get("last_fingerprint") == fingerprint
            and now - int(state.get("last_attempted_at", 0)) < cooldown
        ):
            log("duplicate failure fingerprint is within recovery cooldown")
            return 2

        targets = requested_targets(status)
        incident_id = (
            f"{datetime.now(timezone.utc):%Y%m%dT%H%M%SZ}-"
            f"{fingerprint[:12]}"
        )
        report = {
            "schema_version": 1,
            "incident_id": incident_id,
            "fingerprint": fingerprint,
            "created_at": datetime.now(timezone.utc)
            .isoformat()
            .replace("+00:00", "Z"),
            "requested_targets": targets,
            "actions": [],
            "outcome": "running",
        }
        report_file = state_dir / f"recovery-{incident_id}.json"
        state.update(
            {
                "last_fingerprint": fingerprint,
                "last_attempted_at": now,
                "last_outcome": "running",
            }
        )
        write_json(state_file, state)

        if deployment_active():
            record_event(
                report,
                "deployment_guard",
                "refused",
                "a deployment transaction is active",
            )
            report["outcome"] = "ineligible"
            write_json(report_file, report)
            state["last_outcome"] = "ineligible"
            write_json(state_file, state)
            return 2

        attempted = False
        caddy_needed = False
        for target in targets:
            if target in API_TARGETS:
                api_attempted, proxy_needed = recover_api(target, report)
                attempted = attempted or api_attempted
                caddy_needed = caddy_needed or proxy_needed

        if caddy_needed:
            attempted = True
            recover_caddy(report, status)

        if "backup" in targets:
            attempted = True
            recover_backup(report)

        if not attempted:
            report["outcome"] = "ineligible"
            write_json(report_file, report)
            state["last_outcome"] = "ineligible"
            write_json(state_file, state)
            prune_reports(state_dir)
            return 2

        verification_code = run_full_monitor()
        report["verification_returncode"] = verification_code
        if verification_code == 0:
            command_action(
                report,
                "monitor_reset_failed",
                [
                    "/bin/systemctl",
                    "reset-failed",
                    "verdaxis-monitor.service",
                ],
            )
            report["outcome"] = "recovered"
            state["last_outcome"] = "recovered"
            state["last_recovered_at"] = int(time.time())
            write_json(report_file, report)
            write_json(state_file, state)
            action_names = ", ".join(
                action["name"] for action in report["actions"]
            )
            telegram_send(
                "Verdaxis automatically recovered\n"
                f"Incident: {incident_id}\n"
                f"Actions: {action_names}\n"
                "The full public monitor is passing."
            )
            prune_reports(state_dir)
            return 0

        report["outcome"] = "failed"
        state["last_outcome"] = "failed"
        write_json(report_file, report)
        write_json(state_file, state)
        prune_reports(state_dir)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

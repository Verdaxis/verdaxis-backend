#!/usr/bin/env python3
"""Verdaxis local route and uptime monitor."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path


REQUIRED_HOSTS = [
    "verdaxis.exchange",
    "www.verdaxis.exchange",
    "app.verdaxis.exchange",
    "api.verdaxis.exchange",
    "staging.verdaxis.exchange",
    "api-staging.verdaxis.exchange",
    "deck.verdaxis.exchange",
]

ENDPOINTS = [
    {
        "name": "verdaxis landing",
        "url": "https://verdaxis.exchange/",
        "status": 200,
        "allow_vercel_challenge": True,
    },
    {
        "name": "verdaxis app",
        "url": "https://app.verdaxis.exchange/app",
        "status": 200,
        "allow_vercel_challenge": True,
    },
    {
        "name": "verdaxis staging",
        "url": "https://staging.verdaxis.exchange/",
        "status": 200,
    },
    {
        "name": "verdaxis api",
        "url": "https://api.verdaxis.exchange/health",
        "status": 200,
        "contains": '"status":"ok"',
    },
    {
        "name": "verdaxis staging api",
        "url": "https://api-staging.verdaxis.exchange/health",
        "status": 200,
        "contains": '"status":"ok"',
    },
    {
        "name": "verdaxis staging analytics collector",
        "url": "https://api-staging.verdaxis.exchange/script.js",
        "status": 200,
        "contains": "api/send",
    },
    {
        "name": "verdaxis deck",
        "url": "https://deck.verdaxis.exchange/",
        "status": 200,
        "allow_vercel_challenge": True,
    },
    {
        "name": "verdaxis buyer deck",
        "url": "https://deck.verdaxis.exchange/buyer",
        "status": 200,
        "allow_vercel_challenge": True,
    },
    {
        "name": "verdaxis supplier deck",
        "url": "https://deck.verdaxis.exchange/supplier",
        "status": 200,
        "allow_vercel_challenge": True,
    },
]

DEFAULT_STATE_FILE = "/var/lib/verdaxis-monitor/state.json"
DEFAULT_STATUS_FILE = "/var/lib/verdaxis-monitor/status.json"
DEFAULT_CADDYFILE = "/etc/caddy/Caddyfile"
DEFAULT_MIN_CADDY_BYTES = 20_000
DEFAULT_ALERT_COOLDOWN_SECONDS = 3_600
DEFAULT_HTTP_RETRIES = 3
DEFAULT_HTTP_RETRY_DELAY_SECONDS = 2.0
DEFAULT_RENDER_TIMEOUT_SECONDS = 75
DEFAULT_RENDER_RETRIES = 3
DEFAULT_RENDER_RETRY_DELAY_SECONDS = 2.0
DEFAULT_SIGNUP_CANARY_TARGETS = {
    "prod": "https://api.verdaxis.exchange/api",
    "staging": "https://api-staging.verdaxis.exchange/api",
}
DEFAULT_ANALYTICS_WEBSITE_ID = "9ade1929-3ea9-4660-b352-b8d74f05266a"
DEFAULT_ANALYTICS_DATA_DIR = "/home/verdaxis-prod/verdaxis/analytics/data"
DEFAULT_ANALYTICS_MAX_DATA_BYTES = 5 * 1024 * 1024 * 1024
DEFAULT_MIN_DISK_FREE_PERCENT = 10.0
DEFAULT_BACKUP_STATUS_FILE = "/home/verdaxis-prod/backups/status.json"
DEFAULT_BACKUP_MAX_AGE_SECONDS = 30 * 60 * 60
DEFAULT_OUTBOX_BACKLOG_PROBE = (
    "/usr/local/libexec/verdaxis-monitor/outbox_backlog_probe.py"
)
OUTBOX_BACKLOG_TARGETS = (
    (
        "production",
        "host=127.0.0.1 port=5432 dbname=verdaxis user=verdaxis_backup",
    ),
    (
        "staging",
        "host=127.0.0.1 port=5432 dbname=verdaxis_staging "
        "user=verdaxis_backup_staging",
    ),
)
OUTBOX_MAX_PENDING = 1_000
OUTBOX_MAX_AGE_SECONDS = 300
OUTBOX_QUERY_TIMEOUT_SECONDS = 20
OUTBOX_MAX_OUTPUT_BYTES = 4_096

FRONTEND_BUNDLE_CHECKS = [
    {
        "name": "verdaxis app production bundle",
        "url": "https://app.verdaxis.exchange/login?lang=en",
        "must_contain": ["https://api.verdaxis.exchange/api"],
        "must_not_contain": [
            "http://localhost:8000/api",
            "https://api-staging.verdaxis.exchange/api",
        ],
    },
    {
        "name": "verdaxis staging bundle",
        "url": "https://staging.verdaxis.exchange/login?lang=en",
        "must_contain": ["https://api-staging.verdaxis.exchange/api"],
        "must_not_contain": [
            "http://localhost:8000/api",
            "https://api.verdaxis.exchange/api",
        ],
    },
]

RENDERED_PAGE_CHECKS = [
    {
        "name": "verdaxis app login rendered page",
        "url": "https://app.verdaxis.exchange/login?lang=en",
        "must_contain": ["Sign In"],
        "must_not_contain": [
            "Verdaxis is temporarily under maintenance",
            "temporarily under maintenance",
        ],
    },
]


def run(
    cmd: list[str],
    timeout: int = 20,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
        env=env,
    )


def log(message: str) -> None:
    print(message, file=sys.stderr)
    try:
        run(["/usr/bin/systemd-cat", "-t", "verdaxis-monitor", "echo", message], timeout=5)
    except Exception:
        pass


def load_state(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except Exception:
        return {}


def save_state(path: Path, state: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")
    tmp.replace(path)


def check_caddyfile() -> list[str]:
    errors: list[str] = []
    caddyfile = Path(os.getenv("CADDYFILE", DEFAULT_CADDYFILE))
    min_bytes = int(os.getenv("MIN_CADDYFILE_BYTES", str(DEFAULT_MIN_CADDY_BYTES)))

    if not caddyfile.exists():
        return [f"{caddyfile} does not exist"]

    contents = caddyfile.read_text(errors="replace")
    size = caddyfile.stat().st_size
    if "import " not in contents and size < min_bytes:
        errors.append(f"{caddyfile} is only {size} bytes; expected at least {min_bytes}")

    caddy_command = ["/usr/sbin/runuser", "-u", "caddy", "--", "/usr/bin/caddy"]
    validate = run(
        caddy_command + ["validate", "--config", str(caddyfile)],
        timeout=20,
    )
    if validate.returncode != 0:
        details = (validate.stderr or validate.stdout).strip().splitlines()
        errors.append("caddy validate failed: " + (details[-1] if details else "unknown error"))
        return errors

    adapt = run(
        caddy_command + ["adapt", "--config", str(caddyfile)],
        timeout=20,
    )
    if adapt.returncode != 0:
        details = (adapt.stderr or adapt.stdout).strip().splitlines()
        errors.append("caddy adapt failed: " + (details[-1] if details else "unknown error"))
        return errors

    try:
        adapted = json.loads(adapt.stdout)
    except json.JSONDecodeError as exc:
        errors.append(f"caddy adapt returned invalid JSON: {exc}")
        return errors

    configured_hosts: set[str] = set()
    servers = adapted.get("apps", {}).get("http", {}).get("servers", {})
    for server in servers.values():
        for route in server.get("routes", []):
            for match in route.get("match", []):
                configured_hosts.update(match.get("host", []))

    missing = [host for host in REQUIRED_HOSTS if host not in configured_hosts]
    if missing:
        errors.append("missing Caddy host blocks: " + ", ".join(missing))

    return errors


def check_endpoint(endpoint: dict) -> tuple[str | None, dict]:
    status = {
        "name": endpoint["name"],
        "url": endpoint["url"],
        "expected_status": endpoint["status"],
        "ok": False,
        "attempts": [],
    }
    cmd = [
        "/usr/bin/curl",
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        os.getenv("HTTP_TIMEOUT_SECONDS", "15"),
        "--dump-header",
        "-",
        "--write-out",
        "\n__VERDAXIS_HTTP_CODE__:%{http_code}",
        endpoint["url"],
    ]
    retries = max(1, int(os.getenv("HTTP_RETRIES", str(DEFAULT_HTTP_RETRIES))))
    retry_delay = max(0.0, float(os.getenv("HTTP_RETRY_DELAY_SECONDS", str(DEFAULT_HTTP_RETRY_DELAY_SECONDS))))
    final_error: str | None = None

    for attempt in range(1, retries + 1):
        result = run(cmd, timeout=int(os.getenv("HTTP_TIMEOUT_SECONDS", "15")) + 5)
        attempt_status: dict = {"attempt": attempt}

        if result.returncode != 0:
            attempt_status["error"] = (result.stderr or result.stdout).strip()
            status["attempts"].append(attempt_status)
            final_error = f"{endpoint['name']} curl failed: {attempt_status['error']}"
        else:
            response, _, code_text = result.stdout.rpartition("\n__VERDAXIS_HTTP_CODE__:")
            try:
                code = int(code_text.strip())
            except ValueError:
                attempt_status["error"] = f"invalid HTTP code: {code_text!r}"
                status["attempts"].append(attempt_status)
                final_error = f"{endpoint['name']} returned {attempt_status['error']}"
            else:
                header_text, body = split_curl_response(response)
                headers = parse_headers(header_text)
                attempt_status["http_status"] = code
                status["http_status"] = code

                if code != endpoint["status"]:
                    if endpoint.get("allow_vercel_challenge") and is_vercel_security_challenge(code, headers, body):
                        attempt_status["ok"] = True
                        attempt_status["note"] = "vercel_security_challenge"
                        status["attempts"].append(attempt_status)
                        status["http_status"] = code
                        status["ok"] = True
                        status["note"] = "Vercel edge is reachable but returned Security Checkpoint to the monitor client"
                        return None, status
                    attempt_status["error"] = f"HTTP {code}, expected {endpoint['status']}"
                    status["attempts"].append(attempt_status)
                    final_error = f"{endpoint['name']} returned {attempt_status['error']}"
                else:
                    expected = endpoint.get("contains")
                    if expected and expected not in body:
                        attempt_status["error"] = f"body did not contain {expected!r}"
                        status["attempts"].append(attempt_status)
                        final_error = f"{endpoint['name']} {attempt_status['error']}"
                    else:
                        attempt_status["ok"] = True
                        status["attempts"].append(attempt_status)
                        status["ok"] = True
                        return None, status

        if attempt < retries:
            log(f"{endpoint['name']} check attempt {attempt}/{retries} failed; retrying: {final_error}")
            time.sleep(retry_delay)

    status["error"] = final_error or "unknown endpoint failure"
    return status["error"], status


def split_curl_response(response: str) -> tuple[str, str]:
    """Split curl --dump-header - output into the final response headers and body."""
    normalized = response.replace("\r\n", "\n")
    header_end = normalized.rfind("\n\n")
    if header_end == -1:
        return "", normalized

    header_blob = normalized[:header_end]
    body = normalized[header_end + 2 :]
    header_start = header_blob.rfind("\nHTTP/")
    if header_start == -1:
        final_headers = header_blob
    else:
        final_headers = header_blob[header_start + 1 :]
    return final_headers, body


def parse_headers(header_text: str) -> dict[str, str]:
    headers: dict[str, str] = {}
    for line in header_text.splitlines():
        if ":" not in line:
            continue
        name, value = line.split(":", 1)
        headers[name.strip().lower()] = value.strip()
    return headers


def is_vercel_security_challenge(code: int, headers: dict[str, str], body: str) -> bool:
    if code != 403:
        return False
    if headers.get("server", "").lower() != "vercel":
        return False
    if headers.get("x-vercel-mitigated", "").lower() != "challenge":
        return False
    return "Vercel Security Checkpoint" in body or "x-vercel-challenge-token" in headers


def looks_like_vercel_security_checkpoint(body: str) -> bool:
    normalized = body.lower()
    return (
        "vercel security checkpoint" in normalized
        or "x-vercel-challenge-token" in normalized
        or "we're verifying your browser" in normalized
        or "we are verifying your browser" in normalized
    )


def check_endpoints() -> tuple[list[str], list[dict]]:
    errors: list[str] = []
    statuses: list[dict] = []
    for endpoint in ENDPOINTS:
        error, status = check_endpoint(endpoint)
        statuses.append(status)
        if error:
            errors.append(error)
    return errors, statuses


def text_request(url: str, *, timeout: int = 20) -> tuple[int, str]:
    req = urllib.request.Request(
        url,
        headers={
            "User-Agent": "VerdaxisMonitor/1.0 (+https://verdaxis.exchange)",
            "Accept": "text/html,application/javascript,text/javascript,*/*",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            return response.status, response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", errors="replace")
    except urllib.error.URLError as exc:
        return 0, str(exc.reason)
    except OSError as exc:
        return 0, str(exc)


def check_frontend_bundles() -> list[str]:
    if os.getenv("FRONTEND_BUNDLE_CHECKS_ENABLED", "1").lower() in {"0", "false", "no", "off"}:
        return []

    errors: list[str] = []
    timeout = int(os.getenv("HTTP_TIMEOUT_SECONDS", "15"))
    for check in FRONTEND_BUNDLE_CHECKS:
        code, html = text_request(check["url"], timeout=timeout)
        if code != 200:
            if code == 403 and looks_like_vercel_security_checkpoint(html):
                log(f"{check['name']} skipped: Vercel returned Security Checkpoint to monitor client")
                continue
            errors.append(f"{check['name']} page returned HTTP {code}, expected 200")
            continue

        scripts = re.findall(r'''(?:src|href)=["']([^"']*?/assets/index-[^"']+\.js)["']''', html)
        if not scripts:
            errors.append(f"{check['name']} could not find the main frontend bundle")
            continue

        bundle_url = urllib.parse.urljoin(check["url"], scripts[-1])
        bundle_code, bundle = text_request(bundle_url, timeout=timeout)
        if bundle_code != 200:
            errors.append(f"{check['name']} bundle returned HTTP {bundle_code}, expected 200")
            continue

        for needle in check.get("must_contain", []):
            if needle not in bundle:
                errors.append(f"{check['name']} bundle missing required text {needle!r}")
        for needle in check.get("must_not_contain", []):
            if needle in bundle:
                errors.append(f"{check['name']} bundle still contains forbidden text {needle!r}")

    return errors


def check_backup_status() -> list[str]:
    """Require a recent, complete backup set with locally valid gzip artifacts."""
    status_file = Path(os.getenv("BACKUP_STATUS_FILE", DEFAULT_BACKUP_STATUS_FILE))
    max_age = int(os.getenv("BACKUP_MAX_AGE_SECONDS", str(DEFAULT_BACKUP_MAX_AGE_SECONDS)))
    try:
        payload = json.loads(status_file.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return [f"backup status is missing: {status_file}"]
    except (OSError, json.JSONDecodeError) as exc:
        return [f"backup status is unreadable: {exc}"]

    if payload.get("ok") is not True:
        return ["last database backup did not complete successfully"]

    expected_databases = {"verdaxis", "verdaxis_staging", "umami"}
    if set(payload.get("databases", [])) != expected_databases:
        return ["last database backup did not include prod, staging, and analytics"]

    try:
        completed_at = datetime.fromisoformat(payload["completed_at"])
        if completed_at.tzinfo is None:
            raise ValueError("timestamp has no timezone")
        age_seconds = datetime.now(timezone.utc).timestamp() - completed_at.timestamp()
    except (KeyError, TypeError, ValueError) as exc:
        return [f"backup status has an invalid completion timestamp: {exc}"]

    if age_seconds < -300:
        return ["backup completion timestamp is in the future"]
    if age_seconds > max_age:
        return [f"database backup is stale ({int(age_seconds // 3600)} hours old)"]

    backup_id = payload.get("backup_id")
    if not isinstance(backup_id, str) or not re.fullmatch(r"\d{8}-\d{6}", backup_id):
        return ["backup status has an invalid backup ID"]

    backup_dir = status_file.parent
    artifacts = (
        backup_dir / f"verdaxis-{backup_id}.sql.gz",
        backup_dir / f"verdaxis-staging-{backup_id}.sql.gz",
        backup_dir / f"umami-{backup_id}.sql.gz",
    )
    errors: list[str] = []
    for artifact in artifacts:
        try:
            if artifact.stat().st_size == 0:
                errors.append(f"backup artifact is empty: {artifact.name}")
                continue
        except FileNotFoundError:
            errors.append(f"backup artifact is missing: {artifact.name}")
            continue
        check = run(["/usr/bin/gzip", "-t", str(artifact)], timeout=20)
        if check.returncode != 0:
            errors.append(f"backup artifact failed gzip validation: {artifact.name}")
    return errors


def check_outbox_backlogs() -> tuple[list[str], list[dict]]:
    """Report a stalled event sequencer in either deployed environment."""
    probe = Path(DEFAULT_OUTBOX_BACKLOG_PROBE)
    try:
        if not probe.is_file():
            return ["event outbox backlog probe is missing"], [
                {
                    "environment": environment,
                    "ok": False,
                    "error": "probe_missing",
                }
                for environment, _ in OUTBOX_BACKLOG_TARGETS
            ]
    except OSError:
        return ["event outbox backlog probe is unavailable"], [
            {
                "environment": environment,
                "ok": False,
                "error": "probe_unavailable",
            }
            for environment, _ in OUTBOX_BACKLOG_TARGETS
        ]

    errors: list[str] = []
    statuses: list[dict] = []
    child_environment = {
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": os.defpath,
    }
    passfile = os.getenv("PGPASSFILE", "").strip()
    if passfile:
        child_environment["PGPASSFILE"] = passfile
    for environment, dsn in OUTBOX_BACKLOG_TARGETS:
        status = {
            "environment": environment,
            "ok": False,
            "max_pending": OUTBOX_MAX_PENDING,
            "max_age_seconds": OUTBOX_MAX_AGE_SECONDS,
        }
        statuses.append(status)
        command = [
            sys.executable,
            str(probe),
            "--dsn",
            dsn,
            "--max-pending",
            str(OUTBOX_MAX_PENDING),
            "--max-age-seconds",
            str(OUTBOX_MAX_AGE_SECONDS),
            "--query-timeout",
            str(OUTBOX_QUERY_TIMEOUT_SECONDS),
        ]
        try:
            result = run(
                command,
                timeout=OUTBOX_QUERY_TIMEOUT_SECONDS + 5,
                env=child_environment,
            )
        except subprocess.TimeoutExpired:
            status["error"] = "probe_timeout"
            errors.append(f"{environment} event outbox backlog probe timed out")
            continue
        except OSError:
            status["error"] = "probe_unavailable"
            errors.append(f"{environment} event outbox backlog probe is unavailable")
            continue

        if result.returncode not in {0, 1}:
            status["error"] = "probe_unavailable"
            errors.append(f"{environment} event outbox backlog probe is unavailable")
            continue
        if (
            not isinstance(result.stdout, str)
            or len(result.stdout.encode()) > OUTBOX_MAX_OUTPUT_BYTES
        ):
            status["error"] = "invalid_output"
            errors.append(f"{environment} event outbox backlog probe returned invalid output")
            continue

        try:
            payload = json.loads(result.stdout)
            pending_count = payload["pending_count"]
            oldest_seconds = payload["oldest_pending_seconds"]
            if (
                not isinstance(payload, dict)
                or type(payload.get("ok")) is not bool
                or type(pending_count) is not int
                or type(oldest_seconds) is not int
                or pending_count < 0
                or oldest_seconds < 0
            ):
                raise ValueError
        except (KeyError, TypeError, ValueError, json.JSONDecodeError):
            status["error"] = "invalid_output"
            errors.append(f"{environment} event outbox backlog probe returned invalid output")
            continue

        status["pending_count"] = pending_count
        status["oldest_pending_seconds"] = oldest_seconds

        breached = (
            pending_count > OUTBOX_MAX_PENDING
            or oldest_seconds > OUTBOX_MAX_AGE_SECONDS
        )
        expected_ok = not breached
        if (
            result.returncode != (1 if breached else 0)
            or payload["ok"] is not expected_ok
        ):
            status["error"] = "invalid_output"
            errors.append(f"{environment} event outbox backlog probe returned invalid output")
            continue
        if breached:
            status["error"] = "threshold_breached"
            errors.append(f"{environment} event outbox backlog threshold breached")
            continue
        status["ok"] = True
    return errors, statuses


def find_chromium() -> str | None:
    configured = os.getenv("CHROMIUM_BIN")
    if configured:
        return configured
    for candidate in ("chromium-browser", "chromium", "google-chrome", "google-chrome-stable"):
        path = shutil.which(candidate)
        if path:
            return path
    return None


def check_rendered_pages() -> list[str]:
    if os.getenv("RENDERED_PAGE_CHECKS_ENABLED", "1").lower() in {"0", "false", "no", "off"}:
        return []

    chromium = find_chromium()
    if not chromium:
        return ["rendered page checks enabled but no Chromium binary was found"]

    errors: list[str] = []
    timeout = int(os.getenv("RENDER_TIMEOUT_SECONDS", str(DEFAULT_RENDER_TIMEOUT_SECONDS)))
    virtual_time_ms = int(os.getenv("RENDER_VIRTUAL_TIME_MS", "7000"))
    retries = max(1, int(os.getenv("RENDER_RETRIES", str(DEFAULT_RENDER_RETRIES))))
    retry_delay = max(
        0.0,
        float(
            os.getenv(
                "RENDER_RETRY_DELAY_SECONDS",
                str(DEFAULT_RENDER_RETRY_DELAY_SECONDS),
            )
        ),
    )

    for check in RENDERED_PAGE_CHECKS:
        final_error: str | None = None
        for attempt in range(1, retries + 1):
            try:
                with tempfile.TemporaryDirectory(prefix="verdaxis-monitor-chrome-") as profile:
                    result = run(
                        [
                            chromium,
                            "--headless",
                            "--no-sandbox",
                            "--disable-gpu",
                            "--disable-dev-shm-usage",
                            "--disable-background-networking",
                            "--disable-extensions",
                            "--disable-sync",
                            "--run-all-compositor-stages-before-draw",
                            f"--user-data-dir={profile}",
                            f"--virtual-time-budget={virtual_time_ms}",
                            "--dump-dom",
                            check["url"],
                        ],
                        timeout=timeout,
                    )
            except subprocess.TimeoutExpired:
                final_error = f"{check['name']} render timed out after {timeout}s"
            else:
                if result.returncode != 0:
                    detail = (result.stderr or result.stdout).strip().splitlines()
                    final_error = (
                        f"{check['name']} render failed: "
                        f"{detail[-1] if detail else 'unknown Chromium error'}"
                    )
                elif looks_like_vercel_security_checkpoint(result.stdout):
                    log(
                        f"{check['name']} skipped: Vercel returned Security "
                        "Checkpoint to monitor client"
                    )
                    final_error = None
                    break
                else:
                    rendered_lower = result.stdout.lower()
                    violations = [
                        f"missing required text {needle!r}"
                        for needle in check.get("must_contain", [])
                        if needle.lower() not in rendered_lower
                    ]
                    violations.extend(
                        f"contains forbidden text {needle!r}"
                        for needle in check.get("must_not_contain", [])
                        if needle.lower() in rendered_lower
                    )
                    if not violations:
                        final_error = None
                        break
                    final_error = f"{check['name']} rendered page " + "; ".join(violations)

            if attempt < retries:
                log(
                    f"{check['name']} render attempt {attempt}/{retries} failed; "
                    f"retrying: {final_error}"
                )
                time.sleep(retry_delay)

        if final_error:
            errors.append(final_error)

    return errors


def json_request(url: str, payload: dict, *, headers: dict[str, str] | None = None, timeout: int = 20) -> tuple[int, str]:
    request_headers = {"Content-Type": "application/json", **(headers or {})}
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers=request_headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            return response.status, response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", errors="replace")
    except urllib.error.URLError as exc:
        return 0, str(exc.reason)
    except OSError as exc:
        return 0, str(exc)


def check_analytics_collector() -> list[str]:
    """Exercise the public ingestion path, then remove the synthetic event."""
    if os.getenv("ANALYTICS_CANARY_ENABLED", "1").lower() in {"0", "false", "no", "off"}:
        return []

    website_id = os.getenv("ANALYTICS_WEBSITE_ID", DEFAULT_ANALYTICS_WEBSITE_ID)
    stamp = int(time.time())
    payload = {
        "type": "event",
        "payload": {
            "website": website_id,
            "hostname": "staging.verdaxis.exchange",
            "screen": "1280x720",
            "language": "en-US",
            "title": "Analytics health canary",
            "url": f"/__monitor/analytics/{stamp}",
            "name": "analytics_health_canary",
            "data": {"surface": "monitor"},
        },
    }
    code, body = json_request(
        "https://api-staging.verdaxis.exchange/api/send",
        payload,
        headers={
            "Origin": "https://staging.verdaxis.exchange",
            "User-Agent": (
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
                "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
            ),
        },
    )
    if code != 200:
        return [f"analytics ingestion canary returned HTTP {code}: {body[:200]}"]

    try:
        session_id = json.loads(body)["sessionId"]
    except (KeyError, TypeError, json.JSONDecodeError):
        return [f"analytics ingestion canary returned unexpected body: {body[:200]}"]
    if not re.fullmatch(r"[0-9a-fA-F-]{36}", session_id):
        return ["analytics ingestion canary returned an invalid session ID"]

    cleanup_sql = f"""
        BEGIN;
        DELETE FROM event_data WHERE website_event_id IN (
            SELECT event_id FROM website_event
            WHERE session_id = '{session_id}' AND event_name = 'analytics_health_canary'
        );
        DELETE FROM website_event
            WHERE session_id = '{session_id}' AND event_name = 'analytics_health_canary';
        DELETE FROM session_data WHERE session_id = '{session_id}';
        DELETE FROM session WHERE session_id = '{session_id}'
            AND NOT EXISTS (
                SELECT 1 FROM website_event WHERE session_id = '{session_id}'
            );
        COMMIT;
    """
    cleanup = run(
        [
            "/usr/bin/docker",
            "exec",
            "analytics-db-1",
            "psql",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            "umami",
            "-d",
            "umami",
            "-c",
            cleanup_sql,
        ],
        timeout=20,
    )
    if cleanup.returncode != 0:
        detail = (cleanup.stderr or cleanup.stdout).strip().splitlines()
        return [
            "analytics ingestion canary cleanup failed: "
            + (detail[-1] if detail else "unknown database error")
        ]
    return []


def check_analytics_storage() -> list[str]:
    errors: list[str] = []
    data_dir = Path(os.getenv("ANALYTICS_DATA_DIR", DEFAULT_ANALYTICS_DATA_DIR))
    if not data_dir.exists():
        return [f"analytics data directory is missing: {data_dir}"]

    max_bytes = int(os.getenv("ANALYTICS_MAX_DATA_BYTES", str(DEFAULT_ANALYTICS_MAX_DATA_BYTES)))
    used_bytes = sum(path.stat().st_size for path in data_dir.rglob("*") if path.is_file())
    if used_bytes > max_bytes:
        errors.append(
            f"analytics data is {used_bytes} bytes; configured limit is {max_bytes} bytes"
        )

    disk = shutil.disk_usage(data_dir)
    free_percent = (disk.free / disk.total) * 100 if disk.total else 0.0
    min_free = float(os.getenv("MIN_DISK_FREE_PERCENT", str(DEFAULT_MIN_DISK_FREE_PERCENT)))
    if free_percent < min_free:
        errors.append(
            f"analytics volume has {free_percent:.1f}% free; expected at least {min_free:.1f}%"
        )
    return errors


def parse_signup_canary_targets() -> dict[str, str]:
    raw = os.getenv("SIGNUP_CANARY_TARGETS", "")
    if not raw.strip():
        return DEFAULT_SIGNUP_CANARY_TARGETS
    targets: dict[str, str] = {}
    for item in raw.split(","):
        if not item.strip() or "=" not in item:
            continue
        name, url = item.split("=", 1)
        targets[name.strip()] = url.strip().rstrip("/")
    return targets


def signup_canary(name: str, api_base: str) -> str | None:
    token = os.getenv("MONITOR_TOKEN")
    if not token:
        return f"{name} signup canary missing MONITOR_TOKEN"

    stamp = int(time.time())
    domain = f"{name}-{stamp}.canary.verdaxis.exchange"
    email = f"canary+{name}-{stamp}@{domain}"
    password = f"VerdaxisCanary-{stamp}-check"
    cleanup_error: str | None = None

    try:
        monitor_headers = {"X-Monitor-Token": token}
        code, body = json_request(
            f"{api_base}/auth/register",
            {
                "email": email,
                "password": password,
                "first_name": "Signup",
                "last_name": "Canary",
                "role": "BUYER",
            },
            headers=monitor_headers,
        )
        if code != 200:
            return f"{name} signup canary register returned HTTP {code}: {body[:200]}"
        data = json.loads(body)
        if data.get("status") != "requires_org" or not data.get("registration_token"):
            return f"{name} signup canary register returned unexpected body: {body[:200]}"

        code, body = json_request(
            f"{api_base}/auth/register-with-org",
            {
                "registration_token": data["registration_token"],
                "organization": {
                    "name": f"Verdaxis Canary {name} {stamp}",
                    "type": "FUEL_BUYER",
                    "country_code": "SG",
                    "tax_id": None,
                },
            },
            headers=monitor_headers,
        )
        if code != 200:
            return f"{name} signup canary register-with-org returned HTTP {code}: {body[:200]}"
        data = json.loads(body)
        if data.get("email") != email or data.get("status") != "PENDING":
            return f"{name} signup canary register-with-org returned unexpected body: {body[:200]}"
        return None
    except Exception as exc:
        return f"{name} signup canary failed: {exc}"
    finally:
        code, body = json_request(
            f"{api_base}/monitor/signup-canary-cleanup",
            {"email": email},
            headers={"X-Monitor-Token": token},
        )
        if code != 200:
            cleanup_error = f"{name} signup canary cleanup returned HTTP {code}: {body[:200]}"
        if cleanup_error:
            log(cleanup_error)


def check_signup_canaries() -> list[str]:
    if os.getenv("SIGNUP_CANARY_ENABLED", "1").lower() in {"0", "false", "no", "off"}:
        return []
    errors: list[str] = []
    for name, api_base in parse_signup_canary_targets().items():
        error = signup_canary(name, api_base)
        if error:
            errors.append(error)
    return errors


def write_status(
    ok: bool,
    errors: list[str],
    endpoint_statuses: list[dict],
    outbox_statuses: list[dict] | None = None,
) -> None:
    status_file = Path(os.getenv("STATUS_FILE", DEFAULT_STATUS_FILE))
    now = int(time.time())
    payload = {
        "ok": ok,
        "checked_at": now,
        "checked_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now)),
        "errors": errors,
        "endpoints": endpoint_statuses,
        "outbox_backlogs": outbox_statuses or [],
        "alert_cooldown_seconds": int(
            os.getenv("ALERT_COOLDOWN_SECONDS", str(DEFAULT_ALERT_COOLDOWN_SECONDS))
        ),
    }
    save_state(status_file, payload)


def telegram_alert(message: str) -> None:
    if os.getenv("TELEGRAM_DISABLED", "").lower() in {
        "1",
        "true",
        "yes",
        "on",
    }:
        return
    token = os.getenv("TELEGRAM_BOT_TOKEN")
    chat_id = os.getenv("TELEGRAM_CHAT_ID")
    if not token or not chat_id:
        return

    data = urllib.parse.urlencode({"chat_id": chat_id, "text": message}).encode()
    url = f"https://api.telegram.org/bot{token}/sendMessage"
    req = urllib.request.Request(url, data=data, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            response.read()
    except Exception as exc:
        log(f"telegram alert failed: {str(exc).replace(token, '[redacted]')}")


def healthchecks_ping(ok: bool, message: str) -> None:
    base = os.getenv("HEALTHCHECKS_PING_URL")
    if not base:
        return
    url = base if ok else base.rstrip("/") + "/fail"
    data = message.encode("utf-8")
    req = urllib.request.Request(url, data=data, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            response.read()
    except Exception as exc:
        log(f"healthchecks ping failed: {str(exc).replace(base, '[redacted]')}")


def maybe_alert(ok: bool, errors: list[str]) -> None:
    state_file = Path(os.getenv("STATE_FILE", DEFAULT_STATE_FILE))
    cooldown = int(os.getenv("ALERT_COOLDOWN_SECONDS", str(DEFAULT_ALERT_COOLDOWN_SECONDS)))
    state = load_state(state_file)
    previous_ok = state.get("ok")
    now = int(time.time())
    last_alert_at = int(state.get("last_alert_at", 0))

    if ok:
        message = "Verdaxis monitor recovered: all checks passing"
        if previous_ok is False:
            log(message)
            telegram_alert(message)
        healthchecks_ping(True, "ok")
        save_state(state_file, {"ok": True, "last_checked_at": now, "last_alert_at": last_alert_at})
        return

    message = "Verdaxis monitor failure:\n" + "\n".join(f"- {error}" for error in errors)
    should_alert = previous_ok is not False or (now - last_alert_at) >= cooldown
    log(message)
    if should_alert:
        telegram_alert(message)
        healthchecks_ping(False, message)
        last_alert_at = now

    save_state(
        state_file,
        {
            "ok": False,
            "last_checked_at": now,
            "last_alert_at": last_alert_at,
            "errors": errors,
        },
    )


def main() -> int:
    errors = check_caddyfile()
    endpoint_errors, endpoint_statuses = check_endpoints()
    outbox_statuses: list[dict] = []
    errors.extend(endpoint_errors)
    try:
        errors.extend(check_frontend_bundles())
    except Exception as exc:
        errors.append(f"frontend bundle checks crashed: {exc}")
    try:
        errors.extend(check_rendered_pages())
    except Exception as exc:
        errors.append(f"rendered page checks crashed: {exc}")
    try:
        errors.extend(check_backup_status())
    except Exception as exc:
        errors.append(f"backup status check crashed: {exc}")
    try:
        outbox_errors, outbox_statuses = check_outbox_backlogs()
        errors.extend(outbox_errors)
    except Exception:
        errors.append("event outbox backlog checks crashed")
    try:
        errors.extend(check_signup_canaries())
    except Exception as exc:
        errors.append(f"signup canaries crashed: {exc}")
    try:
        errors.extend(check_analytics_collector())
    except Exception as exc:
        errors.append(f"analytics ingestion canary crashed: {exc}")
    try:
        errors.extend(check_analytics_storage())
    except Exception as exc:
        errors.append(f"analytics storage check crashed: {exc}")
    ok = not errors
    write_status(ok, errors, endpoint_statuses, outbox_statuses)
    maybe_alert(ok, errors)
    if ok:
        print("Verdaxis monitor OK")
        return 0
    return 2


if __name__ == "__main__":
    raise SystemExit(main())

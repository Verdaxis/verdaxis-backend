from __future__ import annotations

import importlib.util
import os
import subprocess
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/external_monitor/verdaxis_monitor.py"


def load_module():
    spec = importlib.util.spec_from_file_location("verdaxis_external_monitor", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _signup_responses(module, cleanup_response):
    email = "canary+prod-123@prod-123.canary.verdaxis.exchange"
    responses = iter(
        [
            (200, '{"status":"requires_org","registration_token":"registration-secret"}'),
            (200, f'{{"email":"{email}","status":"PENDING"}}'),
            cleanup_response,
        ]
    )
    calls = []

    def request(url, payload, *, headers=None, timeout=20):
        calls.append((url, payload, headers, timeout))
        return next(responses)

    module.json_request = request
    return calls


def test_signup_canary_requires_exact_cleanup_after_success(monkeypatch):
    module = load_module()
    monkeypatch.setenv("MONITOR_TOKEN", "monitor-secret")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    calls = _signup_responses(
        module,
        (200, '{"deleted_users":1,"deleted_orgs":1}'),
    )

    assert module.signup_canary("prod", "https://api.example/api") is None
    assert len(calls) == 3
    assert calls[-1][0] == "https://api.example/api/monitor/signup-canary-cleanup"


@pytest.mark.parametrize(
    "cleanup_response",
    [
        (500, "cleanup-response-secret"),
        (200, "not-json"),
        (200, '{}'),
        (200, '{"deleted_users":true,"deleted_orgs":1}'),
        (200, '{"deleted_users":-1,"deleted_orgs":1}'),
        (200, '{"deleted_users":2,"deleted_orgs":1}'),
        (200, '{"deleted_users":0,"deleted_orgs":1}'),
    ],
)
def test_signup_canary_fails_closed_on_invalid_cleanup_after_success(
    monkeypatch,
    cleanup_response,
):
    module = load_module()
    monkeypatch.setenv("MONITOR_TOKEN", "monitor-secret")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    calls = _signup_responses(module, cleanup_response)

    error = module.signup_canary("prod", "https://api.example/api")

    assert error is not None
    assert "signup canary cleanup" in error
    assert "cleanup-response-secret" not in error
    assert "registration-secret" not in error
    assert len(calls) == 3


def test_signup_canary_preserves_signup_and_cleanup_failures(monkeypatch):
    module = load_module()
    monkeypatch.setenv("MONITOR_TOKEN", "monitor-secret")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    responses = iter(
        [
            (503, "signup-response-secret"),
            (500, "cleanup-response-secret"),
        ]
    )
    calls = []

    def request(url, payload, *, headers=None, timeout=20):
        calls.append(url)
        return next(responses)

    monkeypatch.setattr(module, "json_request", request)

    error = module.signup_canary("prod", "https://api.example/api")

    assert error == (
        "prod signup canary register returned HTTP 503; "
        "prod signup canary cleanup returned HTTP 500"
    )
    assert "signup-response-secret" not in error
    assert "cleanup-response-secret" not in error
    assert calls == [
        "https://api.example/api/auth/register",
        "https://api.example/api/monitor/signup-canary-cleanup",
    ]


def test_signup_canary_allows_idempotent_cleanup_after_signup_failure(monkeypatch):
    module = load_module()
    monkeypatch.setenv("MONITOR_TOKEN", "monitor-secret")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    responses = iter(
        [
            (503, "signup-response-secret"),
            (200, '{"deleted_users":0,"deleted_orgs":0}'),
        ]
    )
    monkeypatch.setattr(
        module,
        "json_request",
        lambda url, payload, *, headers=None, timeout=20: next(responses),
    )

    assert module.signup_canary("prod", "https://api.example/api") == (
        "prod signup canary register returned HTTP 503"
    )


def test_signup_canary_cleans_up_once_after_signup_exception(monkeypatch):
    module = load_module()
    monkeypatch.setenv("MONITOR_TOKEN", "monitor-secret")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    calls = []

    def request(url, payload, *, headers=None, timeout=20):
        calls.append(url)
        if len(calls) == 1:
            raise RuntimeError("signup-exception-secret")
        return 200, '{"deleted_users":0,"deleted_orgs":0}'

    monkeypatch.setattr(module, "json_request", request)

    assert module.signup_canary("prod", "https://api.example/api") == (
        "prod signup canary failed (RuntimeError)"
    )
    assert calls == [
        "https://api.example/api/auth/register",
        "https://api.example/api/monitor/signup-canary-cleanup",
    ]


def test_outbox_check_invokes_both_deployed_database_targets(monkeypatch, tmp_path):
    module = load_module()
    probe = tmp_path / "outbox_backlog_probe.py"
    probe.write_text("# synthetic probe\n")
    monkeypatch.setattr(module, "DEFAULT_OUTBOX_BACKLOG_PROBE", str(probe))
    commands = []

    monkeypatch.setenv("PGPASSFILE", "/run/credentials/verdaxis-monitor.pgpass")
    monkeypatch.setenv("PGHOST", "untrusted.example")
    monkeypatch.setenv("PGPORT", "6543")
    monkeypatch.setenv("PGSERVICE", "untrusted-service")
    monkeypatch.setenv("PGOPTIONS", "-c search_path=untrusted")
    monkeypatch.setenv("PGPASSWORD", "must-not-reach-child")

    def healthy(command, timeout=20, env=None):
        commands.append((command, timeout, env))
        return subprocess.CompletedProcess(
            command,
            0,
            stdout='{"ok":true,"pending_count":0,"oldest_pending_seconds":0}\n',
            stderr="",
        )

    monkeypatch.setattr(module, "run", healthy)

    assert module.check_outbox_backlogs()[0] == []
    assert [command[0][command[0].index("--dsn") + 1] for command in commands] == [
        "host=127.0.0.1 port=5432 dbname=verdaxis user=verdaxis_backup",
        "host=127.0.0.1 port=5432 dbname=verdaxis_staging "
        "user=verdaxis_backup_staging",
    ]
    assert all(
        "--max-pending" in command and "1000" in command
        for command, _, _ in commands
    )
    assert all(
        "--max-age-seconds" in command and "300" in command
        for command, _, _ in commands
    )
    assert all(
        env
        == {
            "LANG": "C",
            "LC_ALL": "C",
            "PATH": os.defpath,
            "PGPASSFILE": "/run/credentials/verdaxis-monitor.pgpass",
        }
        for _, _, env in commands
    )


def test_outbox_check_reports_backlog_without_child_diagnostics(monkeypatch, tmp_path):
    module = load_module()
    probe = tmp_path / "outbox_backlog_probe.py"
    probe.write_text("# synthetic probe\n")
    monkeypatch.setattr(module, "DEFAULT_OUTBOX_BACKLOG_PROBE", str(probe))

    def run_check(pending_count, oldest_seconds):
        responses = iter(
            [
                subprocess.CompletedProcess(
                    [],
                    1,
                    stdout=(
                        '{"ok":false,'
                        f'"pending_count":{pending_count},'
                        f'"oldest_pending_seconds":{oldest_seconds}}}\n'
                    ),
                    stderr="database diagnostics must not be reported",
                ),
                subprocess.CompletedProcess(
                    [],
                    0,
                    stdout=(
                        '{"ok":true,"pending_count":0,'
                        '"oldest_pending_seconds":0}\n'
                    ),
                    stderr="",
                ),
            ]
        )
        monkeypatch.setattr(
            module,
            "run",
            lambda command, timeout=20, env=None: next(responses),
        )
        return module.check_outbox_backlogs()

    errors, statuses = run_check(1001, 301)
    later_errors, later_statuses = run_check(5000, 900)

    assert errors == later_errors == [
        "production event outbox backlog threshold breached"
    ]
    assert statuses[0] == {
        "environment": "production",
        "ok": False,
        "max_pending": 1000,
        "max_age_seconds": 300,
        "pending_count": 1001,
        "oldest_pending_seconds": 301,
        "error": "threshold_breached",
    }
    assert later_statuses[0]["pending_count"] == 5000
    assert later_statuses[0]["oldest_pending_seconds"] == 900


def test_outbox_check_fails_closed_on_missing_or_failed_probe(monkeypatch, tmp_path):
    module = load_module()
    missing = tmp_path / "missing-probe.py"
    monkeypatch.setattr(module, "DEFAULT_OUTBOX_BACKLOG_PROBE", str(missing))
    assert module.check_outbox_backlogs() == (
        ["event outbox backlog probe is missing"],
        [
            {
                "environment": "production",
                "ok": False,
                "error": "probe_missing",
            },
            {
                "environment": "staging",
                "ok": False,
                "error": "probe_missing",
            },
        ],
    )

    probe = tmp_path / "outbox_backlog_probe.py"
    probe.write_text("# synthetic probe\n")
    monkeypatch.setattr(module, "DEFAULT_OUTBOX_BACKLOG_PROBE", str(probe))
    monkeypatch.setattr(
        module,
        "run",
        lambda command, timeout=20, env=None: subprocess.CompletedProcess(
            command,
            2,
            stdout='{"ok":false,"error":"password=do-not-report"}\n',
            stderr="postgresql://user:do-not-report@127.0.0.1/verdaxis",
        ),
    )

    errors, statuses = module.check_outbox_backlogs()

    assert errors == [
        "production event outbox backlog probe is unavailable",
        "staging event outbox backlog probe is unavailable",
    ]
    assert "do-not-report" not in " ".join(errors)
    assert [status["error"] for status in statuses] == [
        "probe_unavailable",
        "probe_unavailable",
    ]


def test_main_aggregates_outbox_failures_into_existing_status_path(monkeypatch):
    module = load_module()
    monkeypatch.setattr(module, "check_caddyfile", lambda: [])
    monkeypatch.setattr(module, "check_endpoints", lambda: ([], []))
    monkeypatch.setattr(module, "check_frontend_bundles", lambda: [])
    monkeypatch.setattr(module, "check_rendered_pages", lambda: [])
    monkeypatch.setattr(module, "check_backup_status", lambda: [])
    monkeypatch.setattr(module, "check_signup_canaries", lambda: [])
    monkeypatch.setattr(module, "check_analytics_collector", lambda: [])
    monkeypatch.setattr(module, "check_analytics_storage", lambda: [])
    monkeypatch.setattr(
        module,
        "check_outbox_backlogs",
        lambda: (
            ["production event outbox backlog threshold breached"],
            [
                {
                    "environment": "production",
                    "ok": False,
                    "pending_count": 1001,
                    "oldest_pending_seconds": 301,
                }
            ],
        ),
    )
    observed = {}
    monkeypatch.setattr(
        module,
        "write_status",
        lambda ok, errors, endpoints, outboxes: observed.update(
            ok=ok, errors=errors, endpoints=endpoints, outboxes=outboxes
        ),
    )
    monkeypatch.setattr(module, "maybe_alert", lambda ok, errors: None)

    assert module.main() == 2
    assert observed == {
        "ok": False,
        "errors": ["production event outbox backlog threshold breached"],
        "endpoints": [],
        "outboxes": [
            {
                "environment": "production",
                "ok": False,
                "pending_count": 1001,
                "oldest_pending_seconds": 301,
            }
        ],
    }

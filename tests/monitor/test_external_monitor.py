from __future__ import annotations

import importlib.util
import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/external_monitor/verdaxis_monitor.py"


def load_module():
    spec = importlib.util.spec_from_file_location("verdaxis_external_monitor", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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

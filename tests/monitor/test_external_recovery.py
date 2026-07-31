from __future__ import annotations

import importlib.util
import json
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RECOVERY_PATH = ROOT / "deploy" / "external_monitor" / "verdaxis_recover.py"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def failed_status(*errors: str) -> dict:
    return {
        "ok": False,
        "checked_at": 1,
        "checked_at_utc": "2026-07-31T00:00:00Z",
        "errors": list(errors),
        "endpoints": [
            {
                "name": "verdaxis api",
                "url": "https://api.verdaxis.exchange/health",
                "ok": False,
                "http_status": 502,
                "expected_status": 200,
            },
            {
                "name": "verdaxis staging api",
                "url": "https://api-staging.verdaxis.exchange/health",
                "ok": True,
                "http_status": 200,
                "expected_status": 200,
            },
        ],
    }


def test_recovery_is_allowlisted_and_hourly(tmp_path, monkeypatch):
    recovery = load_module("verdaxis_recover", RECOVERY_PATH)
    status = failed_status(
        "verdaxis api returned HTTP 502, expected 200",
        "database backup is stale (51 hours old)",
        "analytics volume has 8.5% free; expected at least 10.0%",
        "verdaxis app production bundle missing required text",
    )
    assert recovery.requested_targets(status) == ["production_api", "backup"]

    status_file = tmp_path / "status.json"
    state_dir = tmp_path / "state"
    status_file.write_text(json.dumps(status), encoding="utf-8")
    probes = iter(["unavailable", "healthy"])
    commands: list[tuple[str, ...]] = []
    messages: list[str] = []

    monkeypatch.setenv("RECOVERY_STATUS_FILE", str(status_file))
    monkeypatch.setenv("RECOVERY_STATE_DIR", str(state_dir))
    monkeypatch.setattr(recovery, "probe_local", lambda environment: next(probes))
    monkeypatch.setattr(recovery, "probe_public", lambda environment: "healthy")
    monkeypatch.setattr(
        recovery,
        "run_command",
        lambda command, timeout=120: (
            commands.append(tuple(command))
            or subprocess.CompletedProcess(command, 0, "", "")
        ),
    )
    monkeypatch.setattr(recovery, "wait_for_local", lambda environment: True)
    monkeypatch.setattr(recovery, "backup_has_headroom", lambda: True)
    monkeypatch.setattr(recovery, "run_full_monitor", lambda: 0)
    monkeypatch.setattr(recovery, "telegram_send", messages.append)

    assert recovery.main() == 0
    assert recovery.main() == 2
    assert (
        "/bin/systemctl",
        "restart",
        "verdaxis-backend.service",
    ) in commands
    assert (
        "/bin/systemctl",
        "start",
        "verdaxis-backup.service",
    ) in commands
    assert len(messages) == 1
    assert "automatically recovered" in messages[0].lower()

    reports = list(state_dir.glob("recovery-*.json"))
    assert len(reports) == 1
    report = reports[0].read_text(encoding="utf-8")
    assert "frontend" not in report
    assert "analytics volume" not in report


def test_frontend_and_capacity_fail_closed(tmp_path, monkeypatch):
    recovery = load_module("verdaxis_recover_fail_closed", RECOVERY_PATH)
    status_file = tmp_path / "status.json"
    state_dir = tmp_path / "state"
    status_file.write_text(
        json.dumps(
            failed_status(
                "verdaxis app login rendered page missing required text 'Sign In'",
                "analytics volume has 8.5% free; expected at least 10.0%",
            )
        ),
        encoding="utf-8",
    )
    status = json.loads(status_file.read_text(encoding="utf-8"))
    status["endpoints"][0]["ok"] = True
    status_file.write_text(json.dumps(status), encoding="utf-8")

    monkeypatch.setenv("RECOVERY_STATUS_FILE", str(status_file))
    monkeypatch.setenv("RECOVERY_STATE_DIR", str(state_dir))
    monkeypatch.setattr(
        recovery,
        "run_command",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("no command may run")
        ),
    )

    assert recovery.requested_targets(status) == []
    assert recovery.main() == 2


def test_database_and_invalid_caddy_fail_closed(tmp_path, monkeypatch):
    recovery = load_module("verdaxis_recover_database", RECOVERY_PATH)
    status = failed_status(
        "verdaxis api returned HTTP 503, expected 200",
        "caddy validate failed: invalid configuration",
    )
    status_file = tmp_path / "status.json"
    state_dir = tmp_path / "state"
    status_file.write_text(json.dumps(status), encoding="utf-8")
    commands: list[tuple[str, ...]] = []

    monkeypatch.setenv("RECOVERY_STATUS_FILE", str(status_file))
    monkeypatch.setenv("RECOVERY_STATE_DIR", str(state_dir))
    monkeypatch.setattr(
        recovery, "probe_local", lambda environment: "database_unhealthy"
    )
    monkeypatch.setattr(
        recovery,
        "run_command",
        lambda command, timeout=120: (
            commands.append(tuple(command))
            or subprocess.CompletedProcess(command, 0, "", "")
        ),
    )

    assert recovery.caddy_monitor_clean(status) is False
    assert recovery.main() == 2
    assert commands == []


def test_healthy_loopback_uses_one_validated_caddy_reload(
    tmp_path, monkeypatch
):
    recovery = load_module("verdaxis_recover_caddy", RECOVERY_PATH)
    status_file = tmp_path / "status.json"
    state_dir = tmp_path / "state"
    status_file.write_text(
        json.dumps(
            failed_status("verdaxis api returned HTTP 502, expected 200")
        ),
        encoding="utf-8",
    )
    commands: list[tuple[str, ...]] = []

    monkeypatch.setenv("RECOVERY_STATUS_FILE", str(status_file))
    monkeypatch.setenv("RECOVERY_STATE_DIR", str(state_dir))
    monkeypatch.setattr(recovery, "probe_local", lambda environment: "healthy")
    monkeypatch.setattr(
        recovery, "probe_public", lambda environment: "unavailable"
    )
    monkeypatch.setattr(
        recovery,
        "run_command",
        lambda command, timeout=120: (
            commands.append(tuple(command))
            or subprocess.CompletedProcess(command, 0, "", "")
        ),
    )
    monkeypatch.setattr(recovery, "run_full_monitor", lambda: 0)
    monkeypatch.setattr(recovery, "telegram_send", lambda message: None)

    assert recovery.main() == 0
    assert commands.count(
        ("/bin/systemctl", "reload", "caddy.service")
    ) == 1
    assert not any("verdaxis-backend.service" in command for command in commands)


def test_failure_chain_and_external_trigger_contract():
    monitor_unit = (
        ROOT
        / "deploy"
        / "external_monitor"
        / "systemd"
        / "verdaxis-monitor.service"
    ).read_text()
    recovery_unit = (
        ROOT
        / "deploy"
        / "external_monitor"
        / "systemd"
        / "verdaxis-recover.service"
    ).read_text()
    verify_unit = (
        ROOT
        / "deploy"
        / "external_monitor"
        / "systemd"
        / "verdaxis-monitor-verify.service"
    ).read_text()
    external_monitor_unit = (
        ROOT
        / "deploy"
        / "external_monitor"
        / "systemd"
        / "verdaxis-external-monitor.service"
    ).read_text()
    external_recovery_unit = (
        ROOT
        / "deploy"
        / "external_monitor"
        / "systemd"
        / "verdaxis-external-recover.service"
    ).read_text()

    assert "OnFailure=verdaxis-recover.service" in monitor_unit
    assert "OnFailure=verdaxis-codex-diagnose.service" in recovery_unit
    assert "ConditionPathExists=" not in recovery_unit
    assert "OnFailure=" not in verify_unit
    assert "Environment=TELEGRAM_DISABLED=1" in verify_unit
    assert "OnFailure=verdaxis-external-recover.service" in external_monitor_unit
    assert "User=verdaxis-monitor" in external_recovery_unit
    assert "StartLimitIntervalSec=1h" in external_recovery_unit
    assert "StartLimitBurst=1" in external_recovery_unit
    assert "StrictHostKeyChecking=yes" in external_recovery_unit
    assert "PasswordAuthentication=no" in external_recovery_unit
    assert "jons-openclaw@194.233.68.86 true" in external_recovery_unit
    assert "NoNewPrivileges=true" in recovery_unit
    assert "NoNewPrivileges=true" in external_recovery_unit

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy" / "external_monitor" / "verdaxis_autodiag.py"
SPEC = importlib.util.spec_from_file_location("verdaxis_autodiag", MODULE_PATH)
assert SPEC and SPEC.loader
autodiag = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(autodiag)


def test_sync_codex_auth_replaces_stale_dedicated_copy(tmp_path, monkeypatch):
    source = tmp_path / "source-auth.json"
    codex_home = tmp_path / "codex-home"
    codex_home.mkdir()
    target = codex_home / "auth.json"
    stale = {
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": "old-access",
            "id_token": "old-id",
            "refresh_token": "revoked-refresh",
            "account_id": "account",
        },
    }
    current = {
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": "current-access",
            "id_token": "current-id",
            "refresh_token": "current-refresh",
            "account_id": "account",
        },
    }
    source.write_text(json.dumps(current), encoding="utf-8")
    source.chmod(0o600)
    target.write_text(json.dumps(stale), encoding="utf-8")
    target.chmod(0o600)
    monkeypatch.setenv("CODEX_AUTH_SOURCE", str(source))
    monkeypatch.setenv("CODEX_HOME", str(codex_home))

    autodiag.sync_codex_auth()

    assert json.loads(target.read_text(encoding="utf-8")) == current
    assert os.stat(target).st_mode & 0o777 == 0o600


def test_sync_codex_auth_rejects_exposed_source(tmp_path, monkeypatch):
    source = tmp_path / "source-auth.json"
    source.write_text(
        json.dumps(
            {
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "access",
                    "id_token": "id",
                    "refresh_token": "refresh",
                    "account_id": "account",
                },
            }
        ),
        encoding="utf-8",
    )
    source.chmod(0o644)
    monkeypatch.setenv("CODEX_AUTH_SOURCE", str(source))
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / "codex-home"))

    try:
        autodiag.sync_codex_auth()
    except ValueError as exc:
        assert "owner-only" in str(exc)
    else:
        raise AssertionError("exposed Codex auth source was accepted")


def test_healthy_monitor_status_skips_diagnosis(tmp_path, monkeypatch):
    status_file = tmp_path / "monitor-status.json"
    state_dir = tmp_path / "state"
    status_file.write_text(
        json.dumps(
            {
                "ok": True,
                "checked_at": 1,
                "checked_at_utc": "2026-08-08T00:00:00Z",
                "errors": [],
                "endpoints": [],
            }
        ),
        encoding="utf-8",
    )

    monkeypatch.setenv("AUTODIAG_STATUS_FILE", str(status_file))
    monkeypatch.setenv("AUTODIAG_STATE_DIR", str(state_dir))
    monkeypatch.setenv("AUTODIAG_COLLECT_COMMANDS", "0")
    monkeypatch.setenv("CODEX_BIN", "/bin/false")
    monkeypatch.setenv("TELEGRAM_DISABLED", "1")

    assert autodiag.main() == 0
    assert not list(state_dir.glob("incident-*.json"))
    assert not (state_dir / "state.json").exists()


def test_git_snapshot_scopes_shared_caddy_status(monkeypatch):
    commands: list[list[str]] = []

    def fake_run(command, timeout=30):
        commands.append(command)
        return {"command": command, "returncode": 0, "output": ""}

    monkeypatch.setattr(autodiag, "run_command", fake_run)
    paths = (
        "Caddyfile",
        "sites-available/050-verdaxis.caddy",
        "sites-enabled/050-verdaxis.caddy",
        "required-hosts.d/all-active-hosts.txt",
    )

    snapshot = autodiag.git_snapshot("/etc/caddy", paths)

    assert autodiag.REPOSITORY_SCOPES[-1] == ("/etc/caddy", paths)
    assert commands[1][-5:] == ["--", *paths]
    assert snapshot["pathspecs"] == list(paths)


def test_same_failure_runs_once_and_redacts_incident(tmp_path, monkeypatch):
    status_file = tmp_path / "monitor-status.json"
    state_dir = tmp_path / "state"
    recovery_state_dir = tmp_path / "recovery-state"
    counter_file = tmp_path / "counter"
    output_schema = tmp_path / "schema.json"
    workspace = tmp_path / "workspace"
    fake_codex = tmp_path / "codex"
    workspace.mkdir()
    output_schema.write_text("{}\n", encoding="utf-8")
    status_file.write_text(
        json.dumps(
            {
                "ok": False,
                "checked_at": 1,
                "checked_at_utc": "2026-07-31T00:00:00Z",
                "errors": ["backend failed password=secret-token"],
                "endpoints": [
                    {
                        "name": "verdaxis api",
                        "url": "https://api.verdaxis.exchange/health?token=secret",
                        "ok": False,
                        "http_status": 502,
                        "expected_status": 200,
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    status_payload = json.loads(status_file.read_text(encoding="utf-8"))
    fingerprint = autodiag.failure_fingerprint(
        autodiag.normalized_monitor_status(status_payload)
    )
    recovery_state_dir.mkdir()
    (recovery_state_dir / "recovery-test.json").write_text(
        json.dumps(
            {
                "incident_id": "recovery-test",
                "fingerprint": fingerprint,
                "outcome": "failed",
                "requested_targets": ["production_api"],
                "verification_returncode": 2,
                "actions": [
                    {
                        "name": "production_api_restart",
                        "returncode": 0,
                        "output": "restart attempted",
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    fake_codex.write_text(
        "#!/usr/bin/env python3\n"
        "import json, pathlib, sys\n"
        f"counter = pathlib.Path({str(counter_file)!r})\n"
        "counter.write_text(str(int(counter.read_text()) + 1) if counter.exists() else '1')\n"
        "output = pathlib.Path(sys.argv[sys.argv.index('--output-last-message') + 1])\n"
        "output.write_text(json.dumps({\n"
        "  'category': 'backend_runtime',\n"
        "  'severity': 'outage',\n"
        "  'confidence': 'high',\n"
        "  'summary': 'The API is unavailable.',\n"
        "  'likely_root_cause': 'The backend process failed.',\n"
        "  'evidence': ['The public API returned 502.'],\n"
        "  'recommended_next_steps': ['Inspect the backend exception.'],\n"
        "  'requires_human_review': True\n"
        "}))\n",
        encoding="utf-8",
    )
    fake_codex.chmod(0o755)

    monkeypatch.setenv("AUTODIAG_STATUS_FILE", str(status_file))
    monkeypatch.setenv("AUTODIAG_STATE_DIR", str(state_dir))
    monkeypatch.setenv(
        "AUTODIAG_RECOVERY_STATE_DIR", str(recovery_state_dir)
    )
    monkeypatch.setenv("AUTODIAG_SCHEMA_FILE", str(output_schema))
    monkeypatch.setenv("AUTODIAG_WORKSPACE", str(workspace))
    monkeypatch.setenv("AUTODIAG_COLLECT_COMMANDS", "0")
    monkeypatch.setenv("CODEX_BIN", str(fake_codex))
    monkeypatch.setenv("CODEX_AUTH_SYNC", "0")
    monkeypatch.setenv("TELEGRAM_DISABLED", "1")

    assert autodiag.main() == 0
    assert autodiag.main() == 0
    assert counter_file.read_text(encoding="utf-8") == "1"

    incident = next(state_dir.glob("incident-*.json")).read_text(encoding="utf-8")
    assert "secret-token" not in incident
    assert "token=secret" not in incident
    assert "[redacted]" in incident
    assert "production_api_restart" in incident

from __future__ import annotations

import importlib.util
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy" / "external_monitor" / "verdaxis_autodiag.py"
SPEC = importlib.util.spec_from_file_location("verdaxis_autodiag", MODULE_PATH)
assert SPEC and SPEC.loader
autodiag = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(autodiag)


def test_same_failure_runs_once_and_redacts_incident(tmp_path, monkeypatch):
    status_file = tmp_path / "monitor-status.json"
    state_dir = tmp_path / "state"
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
    monkeypatch.setenv("AUTODIAG_SCHEMA_FILE", str(output_schema))
    monkeypatch.setenv("AUTODIAG_WORKSPACE", str(workspace))
    monkeypatch.setenv("AUTODIAG_COLLECT_COMMANDS", "0")
    monkeypatch.setenv("CODEX_BIN", str(fake_codex))
    monkeypatch.setenv("TELEGRAM_DISABLED", "1")

    assert autodiag.main() == 0
    assert autodiag.main() == 0
    assert counter_file.read_text(encoding="utf-8") == "1"

    incident = next(state_dir.glob("incident-*.json")).read_text(encoding="utf-8")
    assert "secret-token" not in incident
    assert "token=secret" not in incident
    assert "[redacted]" in incident

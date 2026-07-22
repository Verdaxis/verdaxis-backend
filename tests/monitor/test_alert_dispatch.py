from __future__ import annotations

import importlib.util
import json
import threading
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/alert_dispatch.py"
NOW = datetime(2026, 7, 20, 4, 0, tzinfo=timezone.utc)


def load_module():
    assert MODULE_PATH.exists(), "alert dispatcher has not been implemented"
    spec = importlib.util.spec_from_file_location("alert_dispatch", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class AlertHandler(BaseHTTPRequestHandler):
    requests: list[tuple[str, bytes]] = []
    fail_telegram = False
    fail_healthchecks = False

    def do_POST(self):  # noqa: N802
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        type(self).requests.append((self.path, body))
        status = 200
        if self.fail_telegram and "sendMessage" in self.path:
            status = 500
        if self.fail_healthchecks and "healthchecks" in self.path:
            status = 500
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, _format, *_args):
        return


@pytest.fixture
def alert_server():
    AlertHandler.requests = []
    AlertHandler.fail_telegram = False
    AlertHandler.fail_healthchecks = False
    server = ThreadingHTTPServer(("127.0.0.1", 0), AlertHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        thread.join()


def alert_env(base_url: str) -> dict[str, str]:
    return {
        "TELEGRAM_BOT_TOKEN": "test-token",
        "TELEGRAM_CHAT_ID": "test-chat",
        "TELEGRAM_API_BASE": f"{base_url}/telegram",
        "HEALTHCHECKS_HEALTH_URL": f"{base_url}/healthchecks/health",
        "HEALTHCHECKS_BACKUP_URL": f"{base_url}/healthchecks/backup",
    }


def telegram_requests():
    return [request for request in AlertHandler.requests if "sendMessage" in request[0]]


def healthchecks_requests():
    return [request for request in AlertHandler.requests if "healthchecks" in request[0]]


def test_failure_transition_hourly_repeat_and_recovery_are_deduped(tmp_path, alert_server):
    dispatcher = load_module()
    env = alert_env(alert_server)

    assert dispatcher.process_event("health-failure", tmp_path, NOW, env) == "delivered"
    assert dispatcher.process_event("health-failure", tmp_path, NOW + timedelta(minutes=30), env) == "deduped"
    assert dispatcher.process_event("health-failure", tmp_path, NOW + timedelta(minutes=61), env) == "delivered"
    assert dispatcher.process_event("health-healthy", tmp_path, NOW + timedelta(minutes=62), env) == "delivered"

    messages = [json.loads(body)["text"] for _path, body in telegram_requests()]
    assert len(messages) == 3
    assert "FAILURE" in messages[0]
    assert "REMINDER" in messages[1]
    assert "RECOVERY" in messages[2]
    assert len(healthchecks_requests()) == 3
    state = json.loads((tmp_path / "health.json").read_text())
    assert state["schema_version"] == 3
    receipt_ids = set()
    for destination in ("telegram", "healthchecks"):
        assert set(state["receipts"][destination]) == {"failure", "healthy"}
        for event_state, receipt in state["receipts"][destination].items():
            assert set(receipt) == {"receipt_id", "delivered_at"}
            assert len(receipt["receipt_id"]) == 32
            assert all(character in "0123456789abcdef" for character in receipt["receipt_id"])
            assert receipt["delivered_at"].endswith("Z")
            receipt_ids.add(receipt["receipt_id"])
    assert len(receipt_ids) == 4


def test_backup_failure_uses_its_own_healthchecks_endpoint(tmp_path, alert_server):
    dispatcher = load_module()

    result = dispatcher.process_event("backup-failure", tmp_path, NOW, alert_env(alert_server))

    assert result == "delivered"
    assert any(path == "/healthchecks/backup/fail" for path, _body in AlertHandler.requests)


def test_delivery_failure_is_retryable_and_never_persists_credentials(tmp_path, alert_server):
    dispatcher = load_module()
    env = alert_env(alert_server)
    AlertHandler.fail_telegram = True

    first = dispatcher.process_event("health-failure", tmp_path, NOW, env)
    state_text = (tmp_path / "health.json").read_text()
    AlertHandler.fail_telegram = False
    second = dispatcher.process_event("health-failure", tmp_path, NOW + timedelta(minutes=1), env)

    assert first == "delivery_failed"
    assert second == "delivered"
    assert "test-token" not in state_text
    assert "test-chat" not in state_text


def test_healthchecks_success_receipt_survives_telegram_failure(tmp_path, alert_server):
    dispatcher = load_module()
    env = alert_env(alert_server)
    AlertHandler.fail_telegram = True

    assert dispatcher.process_event("health-failure", tmp_path, NOW, env) == "delivery_failed"
    state = json.loads((tmp_path / "health.json").read_text())
    assert set(state["receipts"]["healthchecks"]["failure"]) == {
        "receipt_id",
        "delivered_at",
    }
    assert "telegram" not in state["receipts"]

    AlertHandler.fail_telegram = False
    assert dispatcher.process_event(
        "health-failure", tmp_path, NOW + timedelta(minutes=1), env
    ) == "delivered"

    assert len(healthchecks_requests()) == 1
    assert len(telegram_requests()) == 2


def test_telegram_success_receipt_survives_healthchecks_failure(tmp_path, alert_server):
    dispatcher = load_module()
    env = alert_env(alert_server)
    AlertHandler.fail_healthchecks = True

    assert dispatcher.process_event("health-failure", tmp_path, NOW, env) == "delivery_failed"
    state = json.loads((tmp_path / "health.json").read_text())
    assert set(state["receipts"]["telegram"]["failure"]) == {
        "receipt_id",
        "delivered_at",
    }
    assert "healthchecks" not in state["receipts"]

    AlertHandler.fail_healthchecks = False
    assert dispatcher.process_event(
        "health-failure", tmp_path, NOW + timedelta(minutes=1), env
    ) == "delivered"

    assert len(telegram_requests()) == 1
    assert len(healthchecks_requests()) == 2


def test_recovery_partial_failure_retries_only_the_failed_destination(
    tmp_path, alert_server
):
    dispatcher = load_module()
    env = alert_env(alert_server)
    assert dispatcher.process_event("health-failure", tmp_path, NOW, env) == "delivered"
    AlertHandler.requests = []
    AlertHandler.fail_telegram = True

    assert dispatcher.process_event(
        "health-healthy", tmp_path, NOW + timedelta(minutes=1), env
    ) == "delivery_failed"
    state = json.loads((tmp_path / "health.json").read_text())
    healthchecks_recovery = state["receipts"]["healthchecks"]["healthy"]
    assert "healthy" not in state["receipts"]["telegram"]

    AlertHandler.fail_telegram = False
    assert dispatcher.process_event(
        "health-healthy", tmp_path, NOW + timedelta(minutes=2), env
    ) == "delivered"
    updated = json.loads((tmp_path / "health.json").read_text())

    assert updated["receipts"]["healthchecks"]["healthy"] == healthchecks_recovery
    assert len(healthchecks_requests()) == 1
    assert len(telegram_requests()) == 2


@pytest.mark.parametrize(
    "malformed",
    [
        [],
        {"state": []},
        {
            "schema_version": 3.0,
            "state": "failure",
            "observed_at": "2026-07-20T03:00:00Z",
            "receipts": {},
        },
        {"schema_version": 3, "state": "failure", "observed_at": None, "receipts": []},
        {
            "schema_version": 3,
            "state": "failure",
            "observed_at": "2026-07-20T03:00:00Z",
            "receipts": {"telegram": {"failure": {"receipt_id": [], "delivered_at": []}}},
        },
        {
            "schema_version": 3,
            "state": "failure",
            "observed_at": "2027-07-20T03:00:00Z",
            "receipts": {
                "telegram": {
                    "failure": {
                        "receipt_id": "a" * 32,
                        "delivered_at": "2027-07-20T03:00:00Z",
                    },
                },
                "healthchecks": {
                    "failure": {
                        "receipt_id": "b" * 32,
                        "delivered_at": "2027-07-20T03:00:00Z",
                    },
                },
            },
        },
    ],
)
def test_malformed_alert_state_is_quarantined_and_never_suppresses_paging(
    tmp_path, alert_server, malformed
):
    dispatcher = load_module()
    (tmp_path / "health.json").write_text(json.dumps(malformed))

    result = dispatcher.process_event(
        "health-failure", tmp_path, NOW, alert_env(alert_server)
    )

    assert result == "delivered"
    assert len(telegram_requests()) == 1
    assert len(healthchecks_requests()) == 1
    assert len(list(tmp_path.glob("health.json.invalid-*"))) == 1
    assert json.loads((tmp_path / "health.json").read_text())["state"] == "failure"


def test_json_decoder_recursion_in_alert_state_is_quarantined_and_pages(
    tmp_path, alert_server, monkeypatch
):
    dispatcher = load_module()
    (tmp_path / "health.json").write_text("[]")
    state_json = dispatcher.load_json.__globals__["json"]

    def fail_decode(_raw, **_kwargs):
        raise RecursionError

    monkeypatch.setattr(state_json, "loads", fail_decode)

    result = dispatcher.process_event(
        "health-failure", tmp_path, NOW, alert_env(alert_server)
    )

    assert result == "delivered"
    assert len(telegram_requests()) == 1
    assert len(healthchecks_requests()) == 1
    assert len(list(tmp_path.glob("health.json.invalid-*"))) == 1


def test_missing_delivery_configuration_fails_closed(tmp_path):
    dispatcher = load_module()

    result = dispatcher.process_event("health-failure", tmp_path, NOW, {})

    assert result == "delivery_unconfigured"


def test_alert_state_with_duplicate_keys_is_quarantined_and_pages(
    tmp_path, alert_server
):
    dispatcher = load_module()
    (tmp_path / "health.json").write_text(
        '{"schema_version":3,"state":"failure","state":"failure",'
        '"observed_at":"2026-07-20T04:00:00Z","receipts":{}}'
    )

    result = dispatcher.process_event(
        "health-failure", tmp_path, NOW, alert_env(alert_server)
    )

    assert result == "delivered"
    assert len(list(tmp_path.glob("health.json.invalid-*"))) == 1


def test_alert_interval_is_fixed_hourly_and_has_no_force_override():
    dispatcher = load_module()

    args = dispatcher.parse_args(["--event", "health-failure"])
    assert not hasattr(args, "repeat_seconds")
    assert not hasattr(args, "force")
    with pytest.raises(SystemExit):
        dispatcher.parse_args(
            ["--event", "health-failure", "--repeat-seconds", "1"]
        )
    with pytest.raises(SystemExit):
        dispatcher.parse_args(["--event", "health-failure", "--force"])

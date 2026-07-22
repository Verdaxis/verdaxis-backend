from __future__ import annotations

import importlib.util
import json
import stat
import threading
import time
from types import SimpleNamespace
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/local_health_check.py"
READINESS_CORPUS = ROOT / "deploy/monitor/runtime-v2-readiness-corpus.json"
RELEASE_SHA = "a" * 40


def load_module():
    assert MODULE_PATH.exists(), "local health monitor has not been implemented"
    spec = importlib.util.spec_from_file_location("local_health_check", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ResponseHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler API
        routes = {
            "/ready": (
                200,
                json.dumps(
                    {
                        "status": "ok",
                        "db": "ok",
                        "environment": "production",
                        "release_sha": RELEASE_SHA,
                    }
                ).encode(),
                {},
            ),
            "/extra": (
                200,
                json.dumps(
                    {
                        "status": "ok",
                        "db": "ok",
                        "environment": "production",
                        "release_sha": RELEASE_SHA,
                        "build_number": 7,
                    }
                ).encode(),
                {},
            ),
            "/secret": (200, b'{"password":"do-not-copy"}', {}),
            "/duplicate": (
                200,
                (
                    '{"status":"ok","db":"ok","environment":"production",'
                    f'"release_sha":"{RELEASE_SHA}","status":"ok"}}'
                ).encode(),
                {},
            ),
            "/large": (200, b"x" * 4096, {}),
            "/redirect": (302, b"redirect", {"Location": "/ready"}),
        }
        if self.path == "/slow":
            time.sleep(0.2)
            route = routes["/ready"]
        elif self.path == "/slow-drip":
            body = routes["/ready"][1]
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            for byte in body:
                try:
                    self.wfile.write(bytes([byte]))
                    self.wfile.flush()
                    time.sleep(0.01)
                except BrokenPipeError:
                    break
            return
        else:
            route = routes.get(self.path, (404, b"missing", {}))
        code, body, headers = route
        self.send_response(code)
        self.send_header("Content-Length", str(len(body)))
        for name, value in headers.items():
            self.send_header(name, value)
        self.end_headers()
        try:
            self.wfile.write(body)
        except BrokenPipeError:
            pass

    def log_message(self, _format, *_args):
        return


@pytest.fixture
def http_server():
    server = ThreadingHTTPServer(("127.0.0.1", 0), ResponseHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        thread.join()


def test_endpoint_inventory_is_exact_and_numeric_loopback_only():
    monitor = load_module()

    assert monitor.ENDPOINTS == (
        ("prod_ready", "http://127.0.0.1:8000/health/ready"),
        ("staging_ready", "http://127.0.0.1:8001/health/ready"),
    )


def test_runtime_identity_config_cannot_swap_endpoint_environments():
    monitor = load_module()

    valid = {
        "prod_ready": ("production", RELEASE_SHA),
        "staging_ready": ("staging", "b" * 40),
    }
    monitor.validate_runtime_identities(valid)

    with pytest.raises(ValueError):
        monitor.validate_runtime_identities(
            {
                "prod_ready": ("staging", RELEASE_SHA),
                "staging_ready": ("production", "b" * 40),
            }
        )


def test_monitor_reads_dedicated_exact_key_identity_files(tmp_path):
    monitor = load_module()
    production = tmp_path / "production.identity"
    staging = tmp_path / "staging.identity"
    production.write_text(f"ENVIRONMENT=production\nRELEASE_SHA={RELEASE_SHA}\n")
    staging.write_text(f"ENVIRONMENT=staging\nRELEASE_SHA={'b' * 40}\n")

    identities = monitor.load_runtime_identities(production, staging)

    assert identities == {
        "prod_ready": ("production", RELEASE_SHA),
        "staging_ready": ("staging", "b" * 40),
    }

    production.write_text(
        f"ENVIRONMENT=production\nRELEASE_SHA={RELEASE_SHA}\nEXTRA=forbidden\n"
    )
    with pytest.raises(ValueError):
        monitor.load_runtime_identities(production, staging)
    with pytest.raises(ValueError):
        monitor.validate_runtime_identities(
            {
                "prod_ready": ("production", "0" * 40),
                "staging_ready": ("staging", "b" * 40),
            }
        )


@pytest.mark.parametrize(
    "raw",
    [
        f"ENVIRONMENT=production\nRELEASE_SHA={RELEASE_SHA}\nEXTRA=forbidden\n",
        f"ENVIRONMENT=production\nENVIRONMENT=production\nRELEASE_SHA={RELEASE_SHA}\n",
        "ENVIRONMENT=production\n",
        f"# comment\nENVIRONMENT=production\nRELEASE_SHA={RELEASE_SHA}\n",
        f"ENVIRONMENT=production\n\nRELEASE_SHA={RELEASE_SHA}\n",
    ],
)
def test_monitor_identity_file_rejects_missing_duplicate_extra_or_non_data_lines(
    tmp_path, raw
):
    monitor = load_module()
    production = tmp_path / "production.identity"
    staging = tmp_path / "staging.identity"
    production.write_text(raw)
    staging.write_text(f"ENVIRONMENT=staging\nRELEASE_SHA={'b' * 40}\n")

    with pytest.raises(ValueError):
        monitor.load_runtime_identities(production, staging)


def test_monitor_identity_file_rejects_symlinks(tmp_path):
    monitor = load_module()
    actual = tmp_path / "actual.identity"
    actual.write_text(f"ENVIRONMENT=production\nRELEASE_SHA={RELEASE_SHA}\n")
    production = tmp_path / "production.identity"
    production.symlink_to(actual)
    staging = tmp_path / "staging.identity"
    staging.write_text(f"ENVIRONMENT=staging\nRELEASE_SHA={'b' * 40}\n")

    with pytest.raises(ValueError):
        monitor.load_runtime_identities(production, staging)


def test_readiness_requires_exact_sanitized_json(http_server):
    monitor = load_module()

    options = ("production", RELEASE_SHA, 0.5, 1024)
    healthy = monitor.check_readiness("test", f"{http_server}/ready", *options)
    extra = monitor.check_readiness("test", f"{http_server}/extra", *options)
    secret = monitor.check_readiness("test", f"{http_server}/secret", *options)
    duplicate = monitor.check_readiness(
        "test", f"{http_server}/duplicate", *options
    )

    assert healthy["category"] == "healthy"
    assert healthy["code"] == "ok"
    assert extra["category"] == "failure"
    assert extra["code"] == "unexpected_json"
    assert secret["code"] == "unexpected_json"
    assert duplicate["code"] == "invalid_json"
    assert "do-not-copy" not in json.dumps(secret)


def test_runtime_v2_readiness_corpus_is_exactly_enforced():
    monitor = load_module()
    corpus = json.loads(READINESS_CORPUS.read_text())

    assert set(corpus) == {"schema_version", "contract", "cases"}
    assert corpus["schema_version"] == 1
    assert corpus["contract"]["required_keys"] == [
        "status",
        "db",
        "environment",
        "release_sha",
    ]
    for case in corpus["cases"]:
        try:
            monitor.validate_readiness_payload(
                case["payload"],
                corpus["contract"]["environment"],
                corpus["contract"]["release_sha"],
            )
        except ValueError:
            accepted = False
        else:
            accepted = True
        assert accepted is case["valid"], case["name"]


@pytest.mark.parametrize(
    ("change", "expected_code"),
    [
        (lambda payload: payload.pop("release_sha"), "unexpected_json"),
        (lambda payload: payload.update(environment="staging"), "identity_mismatch"),
        (lambda payload: payload.update(release_sha="b" * 40), "identity_mismatch"),
        (lambda payload: payload.update(build_number=7), "unexpected_json"),
        (lambda payload: payload.update(extra={"nested": "value"}), "unexpected_json"),
        (lambda payload: payload.update(extra=["nested"]), "unexpected_json"),
        (lambda payload: payload.update({f"extra_{index}": index for index in range(20)}), "unexpected_json"),
    ],
)
def test_readiness_rejects_missing_extra_or_mismatched_runtime_identity(http_server, change, expected_code):
    monitor = load_module()
    payload = {
        "status": "ok",
        "db": "ok",
        "environment": "production",
        "release_sha": RELEASE_SHA,
    }
    change(payload)
    path = "/runtime-mismatch"
    ResponseHandler.runtime_mismatch = json.dumps(payload).encode()

    original = ResponseHandler.do_GET

    def do_get(handler):
        if handler.path == path:
            body = ResponseHandler.runtime_mismatch
            handler.send_response(200)
            handler.send_header("Content-Length", str(len(body)))
            handler.end_headers()
            handler.wfile.write(body)
            return
        original(handler)

    ResponseHandler.do_GET = do_get
    try:
        result = monitor.check_readiness(
            "test", f"{http_server}{path}", "production", RELEASE_SHA, 0.5, 1024
        )
    finally:
        ResponseHandler.do_GET = original

    assert result["code"] == expected_code


def test_redirects_are_rejected(http_server):
    monitor = load_module()

    result = monitor.check_readiness(
        "test", f"{http_server}/redirect", "production", RELEASE_SHA, 0.5, 1024
    )

    assert result["category"] == "failure"
    assert result["code"] == "redirect_rejected"


def test_response_body_and_per_check_time_are_bounded(http_server):
    monitor = load_module()

    large = monitor.check_readiness(
        "test", f"{http_server}/large", "production", RELEASE_SHA, 0.5, 32
    )
    started = time.monotonic()
    slow = monitor.check_readiness(
        "test", f"{http_server}/slow", "production", RELEASE_SHA, 0.05, 1024
    )

    assert large["code"] == "body_too_large"
    assert slow["code"] == "timeout"
    assert time.monotonic() - started < 0.5


def test_total_http_deadline_stops_a_slow_drip_body(http_server):
    monitor = load_module()
    started = time.monotonic()

    result = monitor.check_readiness(
        "test", f"{http_server}/slow-drip", "production", RELEASE_SHA, 0.08, 1024
    )

    assert result["code"] == "timeout"
    assert time.monotonic() - started < 0.25


class AdvancingClock:
    def __init__(self):
        self.value = 0.0

    def __call__(self):
        return self.value


def test_total_deadline_prevents_later_checks_from_starting():
    monitor = load_module()
    clock = AdvancingClock()
    called = []

    def first(_remaining):
        called.append("first")
        clock.value = 2.0
        return {"name": "first", "category": "healthy", "code": "ok", "duration_ms": 1}

    def second(_remaining):
        called.append("second")
        return {"name": "second", "category": "healthy", "code": "ok", "duration_ms": 1}

    results = monitor.execute_checks((first, second), 1.0, clock=clock)

    assert called == ["first"]
    assert results[-1]["category"] == "failure"
    assert results[-1]["code"] == "total_deadline_exceeded"


@pytest.mark.parametrize(
    ("free_percent", "category", "code"),
    [
        (20.1, "healthy", "ok"),
        (20.0, "warning", "disk_free_warning"),
        (15.1, "warning", "disk_free_warning"),
        (15.0, "failure", "disk_free_critical"),
        (1.0, "failure", "disk_free_critical"),
    ],
)
def test_disk_warning_and_failure_boundaries(free_percent, category, code):
    monitor = load_module()

    def disk_usage(_path):
        total = 10_000
        free = int(total * free_percent / 100)
        return (total, total - free, free)

    result = monitor.check_filesystem("root_disk", "/", 20.0, 15.0, disk_usage=disk_usage)

    assert result["category"] == category
    assert result["code"] == code


def test_optional_analytics_directory_byte_threshold(tmp_path):
    monitor = load_module()
    (tmp_path / "one").write_bytes(b"a" * 4)
    (tmp_path / "two").write_bytes(b"b" * 5)

    healthy = monitor.check_directory_size("analytics", tmp_path, 9, time.monotonic() + 1)
    failure = monitor.check_directory_size("analytics", tmp_path, 8, time.monotonic() + 1)

    assert healthy["category"] == "healthy"
    assert failure["code"] == "directory_size_exceeded"


def test_status_is_atomic_0600_and_warning_is_not_green(tmp_path):
    monitor = load_module()
    path = tmp_path / "status.json"
    previous = {
        "checks": [
            {"name": "root_disk", "last_success_at": "2026-07-19T00:00:00Z"},
        ]
    }
    now = datetime(2026, 7, 20, tzinfo=timezone.utc)
    payload = monitor.build_status(
        [{"name": "root_disk", "category": "warning", "code": "disk_free_warning", "duration_ms": 2}],
        previous,
        now,
    )

    monitor.atomic_write_status(path, payload)

    saved = json.loads(path.read_text())
    assert saved["schema_version"] == 1
    assert saved["generated_at"] == "2026-07-20T00:00:00Z"
    assert saved["category"] == "warning"
    assert saved["checks"][0]["last_success_at"] == "2026-07-19T00:00:00Z"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert not list(tmp_path.glob("*.tmp"))


@pytest.mark.parametrize(("category", "exit_code"), [("healthy", 0), ("warning", 2), ("failure", 2)])
def test_main_pages_warning_and_failure_through_systemd_on_failure(
    tmp_path, monkeypatch, capsys, category, exit_code
):
    monitor = load_module()
    payload = {"schema_version": 1, "generated_at": "2026-07-20T00:00:00Z", "category": category, "checks": []}
    monkeypatch.setattr(monitor, "run_monitor", lambda _args: payload)
    path = tmp_path / "status.json"

    result = monitor.main(["--status-file", str(path)])

    assert result == exit_code
    assert category in capsys.readouterr().out
    assert json.loads(path.read_text())["category"] == category


def test_main_atomically_publishes_unhealthy_status_on_fatal_error(tmp_path, monkeypatch):
    monitor = load_module()
    path = tmp_path / "status.json"
    monkeypatch.setattr(monitor, "run_monitor", lambda _args: (_ for _ in ()).throw(RuntimeError("secret")))
    monkeypatch.setattr(
        monitor,
        "build_status",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("unsafe builder reused")),
    )

    result = monitor.main(["--status-file", str(path)])

    saved = json.loads(path.read_text())
    assert result == 2
    assert saved["category"] == "failure"
    assert saved["checks"][0]["code"] == "fatal_error"
    assert "secret" not in json.dumps(saved)
    assert stat.S_IMODE(path.stat().st_mode) == 0o600


def test_checks_null_state_is_quarantined_and_fails_closed_before_checks(tmp_path):
    monitor = load_module()
    path = tmp_path / "status.json"
    path.write_text('{"schema_version":1,"checks":null}')

    payload = monitor.run_monitor(SimpleNamespace(status_file=str(path)))

    assert payload["category"] == "failure"
    assert payload["checks"][0]["code"] == "previous_status_invalid"
    assert not path.exists()
    assert len(list(tmp_path.glob("status.json.invalid-*"))) == 1


@pytest.mark.parametrize(
    "payload",
    [
        {
            "schema_version": 1,
            "generated_at": "2026-07-20T00:00:00Z",
            "category": "healthy",
            "checks": [
                {
                    "name": "root_disk",
                    "category": "failure",
                    "code": "disk_free_critical",
                    "duration_ms": 1,
                    "last_success_at": None,
                }
            ],
        },
        {
            "schema_version": 1,
            "generated_at": "2026-07-20T00:00:00Z",
            "category": "failure",
            "checks": [],
        },
    ],
)
def test_semantically_inconsistent_persisted_status_is_quarantined_at_monitor_boundary(
    tmp_path, payload
):
    monitor = load_module()
    path = tmp_path / "status.json"
    path.write_text(json.dumps(payload))

    result = monitor.run_monitor(SimpleNamespace(status_file=str(path)))

    assert result["category"] == "failure"
    assert result["checks"][0]["code"] == "previous_status_invalid"
    assert not path.exists()
    assert len(list(tmp_path.glob("status.json.invalid-*"))) == 1


@pytest.mark.parametrize(
    "payload",
    [
        {"schema_version": True, "generated_at": "2026-07-20T00:00:00Z", "category": "healthy", "checks": []},
        {"schema_version": 1, "generated_at": "2026-07-20T00:00:00Z", "category": [], "checks": []},
        {
            "schema_version": 1,
            "generated_at": "2026-07-20T00:00:00Z",
            "category": "healthy",
            "checks": [
                {
                    "name": "root_disk",
                    "category": [],
                    "code": "ok",
                    "duration_ms": 1,
                    "last_success_at": None,
                }
            ],
        },
    ],
)
def test_unhashable_or_boolean_status_types_are_quarantined_without_crash(tmp_path, payload):
    monitor = load_module()
    path = tmp_path / "status.json"
    path.write_text(json.dumps(payload))

    result = monitor.run_monitor(SimpleNamespace(status_file=str(path)))

    assert result["checks"][0]["code"] == "previous_status_invalid"
    assert len(list(tmp_path.glob("status.json.invalid-*"))) == 1


def test_source_has_no_secret_or_privileged_monitor_dependencies():
    source = MODULE_PATH.read_text().lower()

    for forbidden in (
        "telegram",
        "chromium",
        "caddy",
        "docker",
        "monitor_token",
        "database_url",
        "os.environ",
        "os.getenv",
        "verdaxis.exchange",
    ):
        assert forbidden not in source

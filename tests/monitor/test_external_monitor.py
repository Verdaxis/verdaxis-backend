from __future__ import annotations

import importlib.util
import json
import os
import subprocess
from datetime import datetime, timedelta, timezone
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


RESTORE_NOW = datetime(2026, 10, 4, 8, 0, tzinfo=timezone.utc)


def _restore_payload(**changes):
    payload = {
        "ok": True,
        "completed_at": "2026-10-04T07:00:00Z",
        "backup_id": "20261004-070000",
        "databases": ["verdaxis", "verdaxis_staging", "umami"],
    }
    payload.update(changes)
    return payload


def _write_restore_status(monkeypatch, tmp_path, payload):
    backup_status_file = tmp_path / "status.json"
    monkeypatch.setenv("BACKUP_STATUS_FILE", str(backup_status_file))
    restore_status_file = tmp_path / "restore-status.json"
    restore_status_file.write_text(json.dumps(payload))
    return restore_status_file


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


@pytest.mark.parametrize(
    ("response", "expected_error", "secret"),
    [
        (
            (503, "http-response-secret"),
            "analytics ingestion canary returned HTTP 503",
            "http-response-secret",
        ),
        (
            (200, "malformed-json-secret"),
            "analytics ingestion canary returned invalid response",
            "malformed-json-secret",
        ),
        (
            (200, '{"detail":"missing-session-secret"}'),
            "analytics ingestion canary returned invalid response",
            "missing-session-secret",
        ),
        (
            (200, '{"sessionId":null,"detail":"null-session-secret"}'),
            "analytics ingestion canary returned an invalid session ID",
            "null-session-secret",
        ),
        (
            (200, '{"sessionId":123,"detail":"numeric-session-secret"}'),
            "analytics ingestion canary returned an invalid session ID",
            "numeric-session-secret",
        ),
    ],
)
def test_analytics_collector_rejects_responses_without_leaking_or_cleanup(
    monkeypatch,
    response,
    expected_error,
    secret,
):
    module = load_module()
    monkeypatch.setenv("ANALYTICS_CANARY_ENABLED", "1")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    monkeypatch.setattr(
        module,
        "json_request",
        lambda url, payload, *, headers=None, timeout=20: response,
    )
    cleanup_calls = []

    def cleanup(*args, **kwargs):
        cleanup_calls.append((args, kwargs))
        raise AssertionError("cleanup must not run for a rejected response")

    monkeypatch.setattr(module, "run", cleanup)

    errors = module.check_analytics_collector()

    assert errors == [expected_error]
    assert secret not in errors[0]
    assert cleanup_calls == []


@pytest.mark.parametrize("failure_kind", ["nonzero", "timeout"])
def test_analytics_collector_cleanup_failures_do_not_leak(
    monkeypatch,
    failure_kind,
):
    module = load_module()
    monkeypatch.setenv("ANALYTICS_CANARY_ENABLED", "1")
    monkeypatch.setattr(module.time, "time", lambda: 123)
    session_id = "01234567-89ab-cdef-0123-456789abcdef"
    cleanup_secret = "cleanup-subprocess-secret"
    monkeypatch.setattr(
        module,
        "json_request",
        lambda url, payload, *, headers=None, timeout=20: (
            200,
            json.dumps({"sessionId": session_id}),
        ),
    )
    cleanup_calls = []

    def cleanup(command, timeout=20):
        cleanup_calls.append((command, timeout))
        if failure_kind == "timeout":
            raise subprocess.TimeoutExpired(
                command,
                timeout,
                output=cleanup_secret,
                stderr=cleanup_secret,
            )
        return subprocess.CompletedProcess(
            command,
            1,
            stdout=cleanup_secret,
            stderr=cleanup_secret,
        )

    monkeypatch.setattr(module, "run", cleanup)

    errors = module.check_analytics_collector()

    assert errors == ["analytics ingestion canary cleanup failed"]
    assert cleanup_secret not in errors[0]
    assert session_id not in errors[0]
    assert len(cleanup_calls) == 1


def _demo_trade_body(confirmed_at: str, **overrides) -> str:
    item = {
        "id": "sensitive-trade-id",
        "confirmed_at": confirmed_at,
        "is_demo_trade": True,
        "demo_status": "DEMO_ONLY",
        "source_kind": "DEMO_SEED",
        "provenance_kind": "DEMO_SEED",
        **overrides,
    }
    return json.dumps({"items": [item], "body_secret": "must-not-be-reported"})


def test_demo_trade_canary_queries_latest_disclosed_demo_trade(monkeypatch):
    module = load_module()
    now = datetime(2026, 10, 3, 0, 0, tzinfo=timezone.utc)
    calls = []

    def request(url, *, timeout=20):
        calls.append((url, timeout))
        return 200, _demo_trade_body((now - timedelta(minutes=30)).isoformat())

    monkeypatch.setattr(module, "text_request", request)

    assert module.demo_trade_canary(
        "prod",
        "https://api.example/api",
        now=now,
    ) is None
    assert calls == [
        ("https://api.example/api/trade-tape?demo_only=true&limit=1", 20)
    ]


def test_demo_trade_canary_normalizes_utc_z_for_python_310(monkeypatch):
    module = load_module()
    now = datetime(2026, 10, 3, 0, 0, tzinfo=timezone.utc)
    parsed_values = []

    class Python310DateTime:
        @staticmethod
        def fromisoformat(value):
            parsed_values.append(value)
            if value.endswith("Z"):
                raise ValueError("Invalid isoformat string")
            return datetime.fromisoformat(value)

    monkeypatch.setattr(module, "datetime", Python310DateTime)
    monkeypatch.setattr(
        module,
        "text_request",
        lambda url, timeout=20: (200, _demo_trade_body("2026-10-03T00:00:00Z")),
    )

    assert module.demo_trade_canary(
        "prod",
        "https://api.example/api",
        now=now,
    ) is None
    assert parsed_values == ["2026-10-03T00:00:00+00:00"]


@pytest.mark.parametrize(
    ("confirmed_at", "expected_error"),
    [
        ("2026-10-02T23:15:00+00:00", None),
        ("2026-10-02T23:14:59+00:00", "is stale"),
        ("2026-10-03T00:05:00+00:00", None),
        ("2026-10-03T00:05:01+00:00", "too far in the future"),
        ("2026-10-03T00:00:00", "timestamp is not UTC"),
        ("2026-10-03T08:00:00+08:00", "timestamp is not UTC"),
        ("not-a-timestamp", "returned an invalid timestamp"),
    ],
)
def test_demo_trade_canary_enforces_utc_freshness_bounds(
    monkeypatch,
    confirmed_at,
    expected_error,
):
    module = load_module()
    now = datetime(2026, 10, 3, 0, 0, tzinfo=timezone.utc)
    monkeypatch.setattr(
        module,
        "text_request",
        lambda url, timeout=20: (200, _demo_trade_body(confirmed_at)),
    )

    error = module.demo_trade_canary("prod", "https://api.example/api", now=now)

    if expected_error is None:
        assert error is None
    else:
        assert expected_error in error
        assert "sensitive-trade-id" not in error
        assert "must-not-be-reported" not in error


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("is_demo_trade", False),
        ("demo_status", "UNKNOWN"),
        ("source_kind", "CONFIRMED_TRADE"),
        ("provenance_kind", "UNKNOWN"),
    ],
)
def test_demo_trade_canary_requires_all_explicit_demo_fields(
    monkeypatch,
    field,
    value,
):
    module = load_module()
    now = datetime(2026, 10, 3, 0, 0, tzinfo=timezone.utc)
    monkeypatch.setattr(
        module,
        "text_request",
        lambda url, timeout=20: (
            200,
            _demo_trade_body(now.isoformat(), **{field: value}),
        ),
    )

    error = module.demo_trade_canary("prod", "https://api.example/api", now=now)

    assert error == "prod demo trade canary returned invalid DEMO evidence"
    assert "sensitive-trade-id" not in error


@pytest.mark.parametrize(
    ("response", "expected_error"),
    [
        ((503, "body-secret"), "prod demo trade canary returned HTTP 503"),
        ((200, "not-json-body-secret"), "prod demo trade canary returned invalid JSON"),
        (
            (200, '{"items":[]}'),
            "prod demo trade canary returned an invalid item list",
        ),
        (
            (200, '{"items":["trade-id-secret"]}'),
            "prod demo trade canary returned an invalid item list",
        ),
    ],
)
def test_demo_trade_canary_fails_closed_without_response_data(
    monkeypatch,
    response,
    expected_error,
):
    module = load_module()
    monkeypatch.setattr(
        module,
        "text_request",
        lambda url, timeout=20: response,
    )

    error = module.demo_trade_canary(
        "prod",
        "https://api.example/api",
        now=datetime(2026, 10, 3, tzinfo=timezone.utc),
    )

    assert error == expected_error
    assert "secret" not in error


def test_demo_trade_canaries_use_existing_prod_and_staging_targets(monkeypatch):
    module = load_module()
    monkeypatch.delenv("SIGNUP_CANARY_TARGETS", raising=False)
    monkeypatch.delenv("DEMO_TRADE_CANARY_ENABLED", raising=False)
    observed = []
    monkeypatch.setattr(
        module,
        "demo_trade_canary",
        lambda name, api_base: observed.append((name, api_base)),
    )

    assert module.check_demo_trade_canaries() == []
    assert observed == list(module.DEFAULT_SIGNUP_CANARY_TARGETS.items())


@pytest.mark.parametrize(
    ("completed_at", "normalized"),
    [
        ("2026-10-04T07:00:00Z", "2026-10-04T07:00:00+00:00"),
        ("2026-10-04T15:00:00+08:00", "2026-10-04T15:00:00+08:00"),
    ],
)
def test_restore_status_accepts_aware_iso_and_python_310_z(
    monkeypatch,
    tmp_path,
    completed_at,
    normalized,
):
    module = load_module()
    parsed_values = []

    class Python310DateTime:
        @staticmethod
        def now(tz):
            assert tz is timezone.utc
            return RESTORE_NOW

        @staticmethod
        def fromisoformat(value):
            parsed_values.append(value)
            if value.endswith("Z"):
                raise ValueError("Invalid isoformat string")
            return datetime.fromisoformat(value)

    _write_restore_status(
        monkeypatch,
        tmp_path,
        _restore_payload(completed_at=completed_at),
    )
    monkeypatch.setattr(module, "datetime", Python310DateTime)

    assert module.check_restore_status() == []
    assert parsed_values == [normalized]


@pytest.mark.parametrize(
    "payload",
    [
        [],
        {"ok": True, "completed_at": "2026-10-04T07:00:00Z"},
        _restore_payload(unexpected="value"),
        _restore_payload(ok=1),
        _restore_payload(completed_at=1),
        _restore_payload(backup_id=1),
        _restore_payload(databases="verdaxis,verdaxis_staging,umami"),
    ],
)
def test_restore_status_requires_exact_schema(monkeypatch, tmp_path, payload):
    module = load_module()
    _write_restore_status(monkeypatch, tmp_path, payload)

    assert module.check_restore_status() == [
        "restore verification status has an invalid schema"
    ]


@pytest.mark.parametrize(
    "databases",
    [
        ["verdaxis", "verdaxis_staging"],
        ["verdaxis", "verdaxis", "umami"],
        ["verdaxis", "verdaxis_staging", "umami", "other"],
        ["verdaxis", "verdaxis_staging", {"name": "umami"}],
    ],
)
def test_restore_status_requires_exact_database_inventory(
    monkeypatch,
    tmp_path,
    databases,
):
    module = load_module()
    _write_restore_status(
        monkeypatch,
        tmp_path,
        _restore_payload(databases=databases),
    )

    assert module.check_restore_status() == [
        "restore verification status has an invalid database inventory"
    ]


@pytest.mark.parametrize(
    ("changes", "expected_error"),
    [
        (
            {"ok": False},
            "last restore verification did not complete successfully",
        ),
        (
            {"backup_id": "../../secret"},
            "restore verification status has an invalid backup ID",
        ),
        (
            {"completed_at": "not-a-time"},
            "restore verification status has an invalid completion timestamp",
        ),
        (
            {"completed_at": "2026-10-04T07:00:00"},
            "restore verification status has an invalid completion timestamp",
        ),
        (
            {"completed_at": "2026-10-04T08:05:01+00:00"},
            "restore verification completion timestamp is in the future",
        ),
        (
            {"completed_at": "2026-09-26T07:59:59+00:00"},
            "restore verification is stale",
        ),
    ],
)
def test_restore_status_rejects_failed_invalid_and_stale_evidence(
    monkeypatch,
    tmp_path,
    changes,
    expected_error,
):
    module = load_module()
    _write_restore_status(monkeypatch, tmp_path, _restore_payload(**changes))

    class FixedDateTime(datetime):
        @classmethod
        def now(cls, tz=None):
            assert tz is timezone.utc
            return RESTORE_NOW

    monkeypatch.setattr(module, "datetime", FixedDateTime)

    assert module.check_restore_status() == [expected_error]


@pytest.mark.parametrize(
    "raw_payload",
    [
        b'{"secret":"do-not-report"',
        b"x" * 5_001,
    ],
)
def test_restore_status_rejects_unreadable_content_without_details(
    monkeypatch,
    tmp_path,
    raw_payload,
):
    module = load_module()
    status_file = _write_restore_status(
        monkeypatch,
        tmp_path,
        _restore_payload(),
    )
    status_file.write_bytes(raw_payload)

    errors = module.check_restore_status()

    assert errors == ["restore verification status is unreadable"]
    assert "secret" not in " ".join(errors)


def test_restore_status_reports_missing_sibling_without_path(monkeypatch, tmp_path):
    module = load_module()
    backup_status_file = tmp_path / "configured-backup-status.json"
    monkeypatch.setenv("BACKUP_STATUS_FILE", str(backup_status_file))

    errors = module.check_restore_status()

    assert errors == ["restore verification status is missing"]
    assert str(tmp_path) not in " ".join(errors)


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
    monkeypatch.setattr(
        module,
        "check_restore_status",
        lambda: ["restore verification is stale"],
    )
    expensive_status = {
        "ok": False,
        "checked_at": 1_000,
        "checked_at_utc": "1970-01-01T00:16:40Z",
        "errors": ["production demo trade canary is stale"],
    }
    monkeypatch.setattr(
        module,
        "check_expensive_if_due",
        lambda: (["production demo trade canary is stale"], expensive_status),
    )
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
        lambda ok, errors, endpoints, outboxes, expensive: observed.update(
            ok=ok,
            errors=errors,
            endpoints=endpoints,
            outboxes=outboxes,
            expensive=expensive,
        ),
    )
    monkeypatch.setattr(module, "maybe_alert", lambda ok, errors: None)

    assert module.main() == 2
    assert observed == {
        "ok": False,
        "errors": [
            "restore verification is stale",
            "production event outbox backlog threshold breached",
            "production demo trade canary is stale",
        ],
        "endpoints": [],
        "outboxes": [
            {
                "environment": "production",
                "ok": False,
                "pending_count": 1001,
                "oldest_pending_seconds": 301,
            }
        ],
        "expensive": expensive_status,
    }


def test_cached_expensive_failure_stays_active_through_main(monkeypatch, tmp_path):
    module = load_module()
    status_file = tmp_path / "status.json"
    cached = {
        "ok": False,
        "checked_at": 1_000,
        "checked_at_utc": "1970-01-01T00:16:40Z",
        "errors": ["render timed out"],
    }
    module.save_state(status_file, {"expensive_checks": cached})
    monkeypatch.setenv("STATUS_FILE", str(status_file))
    monkeypatch.setenv("EXPENSIVE_CHECK_INTERVAL_SECONDS", "1800")
    monkeypatch.setattr(module.time, "time", lambda: 1_600)
    deep_calls = []
    alerts = []
    monkeypatch.setattr(module, "run_expensive_checks", lambda: deep_calls.append(True))
    monkeypatch.setattr(module, "check_caddyfile", lambda: [])
    monkeypatch.setattr(module, "check_endpoints", lambda: ([], []))
    monkeypatch.setattr(module, "check_restore_status", lambda: [])
    monkeypatch.setattr(module, "check_outbox_backlogs", lambda: ([], []))
    monkeypatch.setattr(
        module,
        "maybe_alert",
        lambda ok, errors: alerts.append((ok, errors)),
    )

    assert module.main() == 2
    assert deep_calls == []
    status = module.load_state(status_file)
    assert status["ok"] is False
    assert status["errors"] == ["render timed out"]
    assert status["expensive_checks"] == cached
    assert alerts == [(False, ["render timed out"])]


@pytest.mark.parametrize(
    ("cached", "interval"),
    [
        ({"errors": []}, "1800"),
        ({"checked_at": "1000", "errors": []}, "1800"),
        ({"checked_at": 1_000, "errors": [1]}, "1800"),
        ({"checked_at": 1_000, "errors": []}, "invalid"),
    ],
)
def test_invalid_expensive_cache_forces_deep_run(
    monkeypatch,
    tmp_path,
    cached,
    interval,
):
    module = load_module()
    status_file = tmp_path / "status.json"
    module.save_state(status_file, {"expensive_checks": cached})
    monkeypatch.setenv("STATUS_FILE", str(status_file))
    monkeypatch.setenv("EXPENSIVE_CHECK_INTERVAL_SECONDS", interval)
    monkeypatch.setattr(module.time, "time", lambda: 1_600)
    deep_calls = []
    monkeypatch.setattr(
        module,
        "run_expensive_checks",
        lambda: deep_calls.append(True) or [],
    )

    errors, status = module.check_expensive_if_due()

    assert errors == []
    assert status["ok"] is True
    assert deep_calls == [True]


def test_expired_expensive_cache_runs_and_records_checks(monkeypatch, tmp_path):
    module = load_module()
    status_file = tmp_path / "status.json"
    module.save_state(
        status_file,
        {"expensive_checks": {"ok": True, "checked_at": 1_000, "errors": []}},
    )
    monkeypatch.setenv("STATUS_FILE", str(status_file))
    monkeypatch.setenv("EXPENSIVE_CHECK_INTERVAL_SECONDS", "1800")
    monkeypatch.setattr(module.time, "time", lambda: 2_800)
    monkeypatch.setattr(
        module,
        "run_expensive_checks",
        lambda: ["backup failed gzip validation"],
    )

    errors, status = module.check_expensive_if_due()

    assert errors == ["backup failed gzip validation"]
    assert status["ok"] is False
    assert status["checked_at"] == 2_800


def test_main_keeps_cheap_checks_on_every_invocation(monkeypatch, tmp_path):
    module = load_module()
    status_file = tmp_path / "status.json"
    monkeypatch.setenv("STATUS_FILE", str(status_file))
    monkeypatch.setattr(module.time, "time", lambda: 10_000)
    calls = {"caddy": 0, "endpoints": 0, "restore": 0, "outboxes": 0, "expensive": 0}

    def called(name, result):
        calls[name] += 1
        return result

    monkeypatch.setattr(module, "check_caddyfile", lambda: called("caddy", []))
    monkeypatch.setattr(
        module,
        "check_endpoints",
        lambda: called("endpoints", ([], [])),
    )
    monkeypatch.setattr(
        module,
        "check_restore_status",
        lambda: called("restore", []),
    )
    monkeypatch.setattr(
        module,
        "check_outbox_backlogs",
        lambda: called("outboxes", ([], [])),
    )
    monkeypatch.setattr(
        module,
        "run_expensive_checks",
        lambda: called("expensive", []),
    )
    monkeypatch.setattr(module, "maybe_alert", lambda ok, errors: None)

    assert module.main() == 0
    assert module.main() == 0
    assert calls == {
        "caddy": 2,
        "endpoints": 2,
        "restore": 2,
        "outboxes": 2,
        "expensive": 1,
    }

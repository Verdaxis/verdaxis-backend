"""Credential runtime preparation and revision policy."""
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("auth_runtime", ROOT / "scripts/prepare_rust_runtime.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


def values():
    return dict(JWT_SECRET="runtime-test-authority-0000000000000000", JWT_ISSUER="test-api",
                JWT_AUDIENCE="test-web", FRONTEND_URL="https://staging.verdaxis.exchange",
                EMAIL_FROM="noreply@example.test", RESEND_API_KEY="disposable-key")


def test_auth_projection_preserves_monitor_authority_and_domain_availability():
    projected = runtime.phase3_settings({**values(), "MONITOR_TOKEN": "disposable-monitor-authority"})
    assert projected["MONITOR_TOKEN"] == "disposable-monitor-authority"
    datasets = json.loads(projected["AUTH_DOMAIN_DATASETS"])
    assert set(datasets) == {"public", "disposable"}
    assert "gmail.com" in datasets["public"]
    assert datasets["disposable"]
    assert "MONITOR_TOKEN" not in runtime.phase3_settings(values())
    with pytest.raises(runtime.RustRuntimeError):
        runtime.phase3_settings({**values(), "MONITOR_TOKEN": 123})


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_auth_checkpoint_is_projected_for_the_exact_release(tmp_path, environment):
    sha = "a" * 40
    role = "verdaxis_app" + ("_staging" if environment == "staging" else "")
    database = "verdaxis_staging" if environment == "staging" else "verdaxis_prod"
    settings = {**values(), "ENVIRONMENT": environment, "RELEASE_SHA": sha,
                "DATABASE_URL": f"postgresql://{role}:disposable@127.0.0.1:5432/{database}"}
    source = tmp_path / "runtime-input.env"
    source.write_text("".join(f"{key}={value}\n" for key, value in settings.items()))
    policy = runtime.load_policy(ROOT, environment)
    projected = runtime.project_runtime_environment(source, sha, "p3_20261020_auth_tokens", policy)
    assert projected["MIGRATION_REVISION"] == "p3_20261020_auth_tokens"
    assert "AUTH_DOMAIN_DATASETS" in projected
    allowed = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert "p3_20261020_auth_tokens" in allowed
    result = runtime.prepare(source, sha, tmp_path / "prepared", ROOT,
                             "p3_20261020_auth_tokens", environment)
    body = result["runtime_env"].read_text()
    assert "AUTH_DOMAIN_DATASETS_FILE=" in body
    domain_file = tmp_path / "prepared/auth-domains.json"
    assert json.loads(domain_file.read_text())["public"]
    assert domain_file.stat().st_mode & 0o777 == 0o600


def test_umami_settings_come_from_the_optional_analytics_file(tmp_path):
    sha = "b" * 40
    settings = {**values(), "ENVIRONMENT": "staging", "RELEASE_SHA": sha,
                "DATABASE_URL": "postgresql://verdaxis_app_staging:disposable@127.0.0.1:5432/verdaxis_staging"}
    source = tmp_path / ".env"
    source.write_text("".join(f"{key}={value}\n" for key, value in settings.items()))
    policy = runtime.load_policy(ROOT, "staging")
    # Without the second file the aggregate provider stays off.
    projected = runtime.project_runtime_environment(source, sha, "sse_20261021_replay_floor", policy)
    assert projected["UMAMI_ENABLED"] == "false"
    # The deployed Python unit reads Umami settings from .env.analytics next to .env.
    analytics = {"UMAMI_BASE_URL": "http://127.0.0.1:8700", "UMAMI_WEBSITE_ID": "website-id",
                 "UMAMI_API_USERNAME": "reader", "UMAMI_API_PASSWORD": "disposable-reader"}
    (tmp_path / ".env.analytics").write_text("".join(f"{key}={value}\n" for key, value in analytics.items()))
    projected = runtime.project_runtime_environment(source, sha, "sse_20261021_replay_floor", policy)
    assert projected["UMAMI_ENABLED"] == "true"
    assert {key: projected[key] for key in analytics} == analytics
    # A link in place of the file is refused, as for .env itself.
    (tmp_path / ".env.analytics").unlink()
    (tmp_path / "elsewhere.env").write_text("UMAMI_BASE_URL=http://127.0.0.1:8700\n")
    (tmp_path / ".env.analytics").symlink_to(tmp_path / "elsewhere.env")
    with pytest.raises(runtime.RustRuntimeError):
        runtime.project_runtime_environment(source, sha, "sse_20261021_replay_floor", policy)


def test_status_heartbeat_settings_are_optional_and_normalized():
    assert runtime.system_status_settings(values()) == {}
    assert runtime.system_status_settings(
        {"SYSTEM_STATUS": "", "SYSTEM_STATUS_MESSAGE": "   "}
    ) == {}
    assert runtime.system_status_settings(
        {"SYSTEM_STATUS": "  Degraded ", "SYSTEM_STATUS_MESSAGE": "  Slow matching  "}
    ) == {"SYSTEM_STATUS": "degraded", "SYSTEM_STATUS_MESSAGE": "Slow matching"}
    for status in ("operational", "degraded", "maintenance"):
        assert runtime.system_status_settings({"SYSTEM_STATUS": status}) == {
            "SYSTEM_STATUS": status
        }


@pytest.mark.parametrize("status", ["ok", "down", "operational degraded", "maintenence"])
def test_status_heartbeat_refuses_an_unknown_status(status):
    with pytest.raises(runtime.RustRuntimeError, match="SYSTEM_STATUS"):
        runtime.system_status_settings({"SYSTEM_STATUS": status})


def test_status_heartbeat_message_limit_is_280_characters():
    at_limit = "x" * 280
    assert runtime.system_status_settings({"SYSTEM_STATUS_MESSAGE": at_limit}) == {
        "SYSTEM_STATUS_MESSAGE": at_limit
    }
    assert runtime.system_status_settings({"SYSTEM_STATUS_MESSAGE": f"  {at_limit} "}) == {
        "SYSTEM_STATUS_MESSAGE": at_limit
    }
    with pytest.raises(runtime.RustRuntimeError, match="SYSTEM_STATUS_MESSAGE"):
        runtime.system_status_settings({"SYSTEM_STATUS_MESSAGE": at_limit + "x"})


def _prepare_with(tmp_path, extra):
    sha = "a" * 40
    settings = {**values(), "ENVIRONMENT": "staging", "RELEASE_SHA": sha,
                "DATABASE_URL": "postgresql://verdaxis_app_staging:disposable"
                                "@127.0.0.1:5432/verdaxis_staging", **extra}
    source = tmp_path / "runtime-input.env"
    source.write_text("".join(f"{key}={value}\n" for key, value in settings.items()))
    return runtime.prepare(source, sha, tmp_path / "prepared", ROOT,
                           "p3_20261020_auth_tokens", "staging")["runtime_env"]


def test_status_heartbeat_settings_reach_the_runtime_file(tmp_path):
    body = _prepare_with(
        tmp_path,
        {"SYSTEM_STATUS": "Maintenance", "SYSTEM_STATUS_MESSAGE": "Back at 14:00 UTC"},
    ).read_text()
    assert "SYSTEM_STATUS=maintenance\n" in body
    assert 'SYSTEM_STATUS_MESSAGE="Back at 14:00 UTC"\n' in body


def test_runtime_file_has_no_status_heartbeat_keys_when_unset(tmp_path):
    body = _prepare_with(tmp_path, {"SYSTEM_STATUS": "", "SYSTEM_STATUS_MESSAGE": ""}).read_text()
    assert "SYSTEM_STATUS" not in body


def test_runtime_file_refuses_an_invalid_status_or_a_message_with_a_line_break(tmp_path):
    with pytest.raises(runtime.RustRuntimeError, match="SYSTEM_STATUS"):
        _prepare_with(tmp_path, {"SYSTEM_STATUS": "broken"})
    # A double-quoted value with an escaped newline is a control character the
    # runtime file cannot carry.
    with pytest.raises(runtime.RustRuntimeError, match="SYSTEM_STATUS_MESSAGE"):
        _prepare_with(tmp_path, {"SYSTEM_STATUS_MESSAGE": '"two\\nlines"'})

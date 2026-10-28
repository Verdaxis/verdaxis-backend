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

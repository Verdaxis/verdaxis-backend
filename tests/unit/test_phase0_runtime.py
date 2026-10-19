"""Fixed canonical runtime identity; no live services are used."""
from pathlib import Path

import pytest
from pydantic import ValidationError

from app.config import Settings
from app.environment_database import expected_database_for_environment
from scripts.converge_runtime_acls import _IDENTITIES as DEPLOYED_IDENTITIES

ROOT = Path(__file__).resolve().parents[2]


def production_settings(database="verdaxis_prod"):
    return Settings(
        _env_file=None, ENVIRONMENT="production", RELEASE_SHA="a" * 40,
        JWT_SECRET="synthetic-secret-that-is-long-enough-for-tests",
        DATABASE_NAME=database, DATABASE_USER="verdaxis_app",
        DATABASE_PASSWORD="synthetic-password",
        DATABASE_URL=f"postgresql+asyncpg://verdaxis_app:synthetic-password@127.0.0.1/{database}",
        MIGRATOR_DATABASE_URL=f"postgresql+asyncpg://verdaxis_migrator:synthetic-password@127.0.0.1/{database}",
    )


def test_all_canonical_identity_consumers_require_new_database():
    assert production_settings().DATABASE_NAME == "verdaxis_prod"
    assert expected_database_for_environment("production", url_database="verdaxis_prod") == "verdaxis_prod"
    assert DEPLOYED_IDENTITIES["production"]["database_name"] == "verdaxis_prod"


@pytest.mark.parametrize("database", ["verdaxis", "another", "verdaxis_staging"])
def test_old_database_is_only_for_old_release(database):
    with pytest.raises(ValidationError):
        production_settings(database)


def test_canonical_branch_and_canary_origin():
    source = (ROOT / "scripts/deploy.sh").read_text()
    # The canonical production checkout follows `prod`, the branch whose pushes
    # publish the production Rust artifact; the staging checkout follows `staging`.
    production = source.split("*/prod/be)", 1)[1].split(";;", 1)[0]
    staging = source.split("*/staging/be)", 1)[1].split(";;", 1)[0]
    assert 'DEFAULT_BRANCH="prod"' in production
    assert 'DEFAULT_BRANCH="staging"' in staging
    assert "https://canary.verdaxis.exchange" in production_settings().BACKEND_CORS_ORIGINS


def test_reminder_units_are_guarded_and_not_activated():
    manifest = (ROOT / "deploy/systemd/runtime-units.manifest").read_text()
    for suffix in ("", "-staging"):
        name = f"verdaxis-order-expiry-reminders{suffix}"
        assert f"{name}.service" in manifest
        assert f"{name}.timer" in manifest
        service = (ROOT / f"deploy/systemd/{name}.service").read_text()
        assert "test ! -e" in service and "test ! -L" in service
        assert "OnCalendar=*:0/15" in (ROOT / f"deploy/systemd/{name}.timer").read_text()
    assert "external-monitor" not in manifest


def test_release_publication_includes_matching_monitor_identity(tmp_path):
    import subprocess
    source = (ROOT / "scripts/deploy.sh").read_text()
    functions = source[source.index("write_monitor_identity() {"):source.index("write_deploy_state() {")]
    script = 'set -euo pipefail\n' + functions + '\nwrite_release_artifact "' + 'a' * 40 + '" p0_20261016_union\n'
    result = subprocess.run(["bash", "-c", script], env={
        "PATH": "/usr/bin:/bin", "BACKEND_DIR": str(tmp_path),
        "RELEASE_ENV_FILE": str(tmp_path / ".runtime-release.env"),
        "DEPLOY_ENVIRONMENT": "production",
    }, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    identity = "ENVIRONMENT=production\nRELEASE_SHA=" + "a" * 40 + "\n"
    assert (tmp_path / ".runtime-monitor.env").read_text() == identity
    assert (tmp_path / ".runtime-release.env").read_text() == identity + "MIGRATION_REVISION=p0_20261016_union\n"


@pytest.mark.parametrize("environment,status", [("production", 200), ("staging", 400)])
def test_canary_credentialed_preflight_is_production_only(environment, status):
    from fastapi import FastAPI
    from starlette.middleware.cors import CORSMiddleware
    from starlette.testclient import TestClient
    from app.config import credentialed_origins_for_environment
    app = FastAPI()
    app.add_middleware(CORSMiddleware, allow_origins=list(credentialed_origins_for_environment(environment)),
                       allow_credentials=True, allow_methods=["GET"])
    with TestClient(app) as client:
        response = client.options("/", headers={"Origin": "https://canary.verdaxis.exchange",
                                                "Access-Control-Request-Method": "GET"})
    assert response.status_code == status
    if status == 200:
        assert response.headers["access-control-allow-origin"] == "https://canary.verdaxis.exchange"
        assert response.headers["access-control-allow-credentials"] == "true"
    else:
        assert "access-control-allow-origin" not in response.headers


def test_seed_paths_explicitly_deny_both_production_names():
    from app.seeds.safety import SeedTargetError, resolve_seed_target
    for database in ("verdaxis", "verdaxis_prod"):
        with pytest.raises(SeedTargetError, match="production"):
            resolve_seed_target({
                "SEED_DATABASE_URL": f"postgresql://verdaxis_migrator:synthetic@127.0.0.1/{database}",
                "SEED_TARGET_DATABASE": database, "SEED_RUNTIME_ENV": "disposable",
                "ALLOW_SEED_MUTATIONS": "I_UNDERSTAND_SEED_MUTATIONS",
            })

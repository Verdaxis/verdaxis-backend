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
    assert 'DEFAULT_BRANCH="prod"' not in source
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

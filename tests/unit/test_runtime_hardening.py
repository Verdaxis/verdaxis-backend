"""Focused tests for runtime configuration and integration safety."""

from pathlib import Path

import pytest
from pydantic import ValidationError

from app.config import Settings
from app.database import engine_options
from tests.runtime_config import (
    REMOTE_MUTATION_OPT_IN,
    RuntimeTestConfigurationError,
    resolve_test_api_url,
)


def _settings(**overrides):
    values = {
        "DATABASE_URL": "sqlite+aiosqlite:///:memory:",
        "JWT_SECRET": "test-secret-key-that-is-at-least-32-characters-long",
        "ENVIRONMENT": "test",
    }
    values.update(overrides)
    return Settings(**values)


def test_pool_defaults_leave_headroom_for_four_workers_on_postgres_max_100():
    settings = _settings()

    assert settings.DB_POOL_SIZE == 5
    assert settings.DB_MAX_OVERFLOW == 2
    assert settings.DB_POOL_WORKERS == 4
    assert settings.DB_MAX_CONNECTIONS == 100
    assert settings.DB_RESERVED_CONNECTIONS == 20
    assert settings.DB_POOL_WORKERS * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) <= (
        settings.DB_MAX_CONNECTIONS - settings.DB_RESERVED_CONNECTIONS
    )


@pytest.mark.parametrize(
    "overrides",
    [
        {"DB_POOL_SIZE": 0},
        {"DB_MAX_OVERFLOW": -1},
        {"DB_POOL_WORKERS": 0},
        {"DB_MAX_CONNECTIONS": 10, "DB_RESERVED_CONNECTIONS": 10},
        {"DB_POOL_WORKERS": 20, "DB_POOL_SIZE": 5, "DB_MAX_OVERFLOW": 2},
    ],
)
def test_pool_settings_reject_invalid_or_unsafe_capacity(overrides):
    with pytest.raises(ValidationError):
        _settings(**overrides)


def test_sqlite_settings_keep_pool_configuration_but_database_layer_can_disable_pooling():
    settings = _settings(DB_POOL_SIZE=3, DB_MAX_OVERFLOW=1)

    assert settings.DATABASE_URL.startswith("sqlite")
    assert settings.DB_POOL_SIZE == 3
    assert engine_options(settings) == {}


def test_postgres_engine_options_use_validated_per_worker_pool_settings():
    settings = _settings(
        DATABASE_URL="postgresql+asyncpg://user:pass@localhost/test",
        DB_POOL_SIZE=6,
        DB_MAX_OVERFLOW=1,
        DB_POOL_TIMEOUT=15,
        DB_POOL_RECYCLE=900,
    )

    assert engine_options(settings) == {
        "pool_size": 6,
        "max_overflow": 1,
        "pool_timeout": 15,
        "pool_pre_ping": True,
        "pool_recycle": 900,
    }


def test_integration_url_requires_explicit_environment_value():
    with pytest.raises(RuntimeTestConfigurationError, match="TEST_API_URL"):
        resolve_test_api_url({})


def test_integration_url_rejects_production_host():
    with pytest.raises(RuntimeTestConfigurationError, match="production"):
        resolve_test_api_url({"TEST_API_URL": "https://api.verdaxis.exchange"})


def test_remote_mutating_target_requires_unmistakable_opt_in():
    env = {"TEST_API_URL": "https://api-staging.verdaxis.exchange"}

    with pytest.raises(RuntimeTestConfigurationError, match="REMOTE_TEST_MUTATIONS"):
        resolve_test_api_url(env, require_mutation_opt_in=True)

    env[REMOTE_MUTATION_OPT_IN] = "I_UNDERSTAND_REMOTE_TEST_MUTATIONS"
    assert resolve_test_api_url(env, require_mutation_opt_in=True) == env["TEST_API_URL"]


def test_local_mutating_target_does_not_need_remote_opt_in():
    assert (
        resolve_test_api_url(
            {"TEST_API_URL": "http://127.0.0.1:18765"},
            require_mutation_opt_in=True,
        )
        == "http://127.0.0.1:18765"
    )


def test_systemd_templates_are_loopback_bound_and_sandboxed():
    root = Path(__file__).parents[2]
    for path in (
        root / "deploy/systemd/verdaxis-backend.service",
        root / "deploy/systemd/verdaxis-backend-staging.service",
    ):
        contents = path.read_text()
        assert "--host 127.0.0.1" in contents
        assert "After=network-online.target docker.service" in contents
        assert "ExecStartPre=" in contents
        assert "KillSignal=SIGTERM" in contents
        assert "TimeoutStopSec=30" in contents
        assert "NoNewPrivileges=true" in contents
        assert "ProtectSystem=strict" in contents
        assert "ProtectHome=read-only" in contents
        assert "PrivateTmp=true" in contents
        assert "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6" in contents


def test_migration_verification_is_upgrade_then_drift_check():
    root = Path(__file__).parents[2]
    script = (root / "scripts/verify_migrations.sh").read_text()

    assert "upgrade head" in script
    assert " check" in script
    assert "DATABASE_URL" in script

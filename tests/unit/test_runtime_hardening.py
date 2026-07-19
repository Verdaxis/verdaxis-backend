"""Focused tests for runtime configuration and integration safety."""

from pathlib import Path

import pytest
from pydantic import ValidationError
from alembic.autogenerate import compare_metadata
from alembic.migration import MigrationContext
from sqlalchemy import Column, ForeignKey, Integer, MetaData, String, Table, create_engine, text
from app.migration_drift import compare_server_default, compare_type, include_object

from app.config import Settings
from app.database import engine_options, migrator_connect_args
from tests.runtime_config import (
    RuntimeTestConfigurationError,
    resolve_test_api_url,
)

MUTATION_OPT_IN = "ALLOW_TEST_MUTATIONS"
MUTATION_OPT_IN_VALUE = "I_UNDERSTAND_TEST_MUTATIONS"
RUNTIME_ENV_ATTESTATION = "TEST_RUNTIME_ENV"
DISPOSABLE_DB_NAME = "TEST_DISPOSABLE_DB_NAME"


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

    assert settings.DB_POOL_SIZE == 2
    assert settings.DB_MAX_OVERFLOW == 1
    assert settings.DB_POOL_WORKERS == 4
    assert settings.DB_SERVICE_COUNT == 2
    assert settings.DB_MAX_CONNECTIONS == 100
    assert settings.DB_RESERVED_CONNECTIONS == 20
    assert settings.DB_SERVICE_COUNT * settings.DB_POOL_WORKERS * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) + settings.DB_RESERVED_CONNECTIONS == 44
    assert settings.DB_SERVICE_COUNT * settings.DB_POOL_WORKERS * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) <= (
        settings.DB_MAX_CONNECTIONS - settings.DB_RESERVED_CONNECTIONS
    )


@pytest.mark.parametrize(
    "unit_name",
    ["verdaxis-backend.service", "verdaxis-backend-staging.service"],
)
def test_deployed_units_use_the_validated_connection_budget(unit_name):
    root = Path(__file__).parents[2]
    service = (root / "deploy/systemd" / unit_name).read_text()
    worker_count = int(service.split("--workers ", 1)[1].split()[0])
    settings = _settings(DB_POOL_WORKERS=worker_count)

    assert worker_count == 4
    assert settings.DB_SERVICE_COUNT * worker_count * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) + settings.DB_RESERVED_CONNECTIONS <= settings.DB_MAX_CONNECTIONS


@pytest.mark.parametrize(
    "overrides",
    [
        {"DB_POOL_SIZE": 0},
        {"DB_MAX_OVERFLOW": -1},
        {"DB_POOL_WORKERS": 0},
        {"DB_MAX_CONNECTIONS": 10, "DB_RESERVED_CONNECTIONS": 10},
        {"DB_POOL_WORKERS": 20, "DB_POOL_SIZE": 5, "DB_MAX_OVERFLOW": 2},
        {"DB_STATEMENT_TIMEOUT_MS": 60_000, "MIGRATOR_STATEMENT_TIMEOUT_MS": 30_000},
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


def test_kyc_upload_defaults_are_bounded_and_aggregate_limit_is_enforced():
    settings = _settings()

    assert settings.KYC_MAX_FILE_BYTES == 10 * 1024 * 1024
    assert settings.KYC_MAX_TOTAL_BYTES == 20 * 1024 * 1024
    with pytest.raises(ValidationError):
        _settings(KYC_MAX_FILE_BYTES=11, KYC_MAX_TOTAL_BYTES=10)


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
        "connect_args": {
            "server_settings": {
                "statement_timeout": "30000",
                "lock_timeout": "3000",
                "idle_in_transaction_session_timeout": "60000",
            }
        },
    }


def test_migrator_policy_is_longer_lived_but_bounded():
    settings = _settings()

    assert settings.MIGRATOR_STATEMENT_TIMEOUT_MS > settings.DB_STATEMENT_TIMEOUT_MS
    assert settings.MIGRATOR_LOCK_TIMEOUT_MS > settings.DB_LOCK_TIMEOUT_MS
    assert settings.MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS > settings.DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS
    assert migrator_connect_args(settings) == {
        "server_settings": {
            "statement_timeout": "300000",
            "lock_timeout": "30000",
            "idle_in_transaction_session_timeout": "300000",
        }
    }


def test_alembic_uses_separate_migrator_url_and_policy():
    root = Path(__file__).parents[2]
    source = (root / "alembic/env.py").read_text()

    assert "MIGRATOR_DATABASE_URL or settings.DATABASE_URL" in source
    assert "connect_args=migrator_connect_args(settings)" in source


def test_integration_url_requires_explicit_environment_value():
    with pytest.raises(RuntimeTestConfigurationError, match="TEST_API_URL"):
        resolve_test_api_url({})


def test_integration_url_rejects_production_host():
    with pytest.raises(RuntimeTestConfigurationError, match="production"):
        resolve_test_api_url({"TEST_API_URL": "https://api.verdaxis.exchange"})


def test_mutating_target_requires_opt_in_and_positive_environment_attestation():
    env = {"TEST_API_URL": "http://127.0.0.1:18765"}

    with pytest.raises(RuntimeTestConfigurationError, match="TEST_MUTATIONS"):
        resolve_test_api_url(env, require_mutation_opt_in=True)

    env[MUTATION_OPT_IN] = MUTATION_OPT_IN_VALUE
    with pytest.raises(RuntimeTestConfigurationError, match="RUNTIME_ENV"):
        resolve_test_api_url(env, require_mutation_opt_in=True)

    env[RUNTIME_ENV_ATTESTATION] = "disposable"
    env[DISPOSABLE_DB_NAME] = "verdaxis_runtime_test"
    assert resolve_test_api_url(env, require_mutation_opt_in=True) == env["TEST_API_URL"]


@pytest.mark.parametrize(
    "url, runtime_env",
    [
        ("http://127.0.0.1:8000", "disposable"),
        ("http://127.0.0.1:8001", "disposable"),
        ("http://localhost:8000", "disposable"),
        ("http://localhost:8001", "disposable"),
        ("http://[::1]:8000", "disposable"),
        ("http://[::1]:8001", "disposable"),
        ("http://144.126.151.136:8000", "staging"),
        ("https://api-staging.verdaxis.exchange:8000", "staging"),
        ("https://api-staging.verdaxis.exchange:8001", "staging"),
        ("https://api.verdaxis.exchange", "staging"),
        ("https://www.verdaxis.exchange", "staging"),
        ("https://api.verdaxis.exchange.", "staging"),
        ("https://api-staging.verdaxis.exchange", "disposable"),
        ("http://api-staging.verdaxis.exchange", "staging"),
        ("https://api-staging.verdaxis.exchange.evil.example", "staging"),
        ("https://api-staging.verdaxis.exchange.", "staging"),
        ("https://api-staging.verdaxis.exchange@127.0.0.1:8001", "staging"),
        ("https://127.0.0.1:8001", "staging"),
        ("https://[::1]:443", "staging"),
        ("https://example.invalid", "disposable"),
    ],
)
def test_mutating_target_rejects_production_or_wrong_attestation(url, runtime_env):
    env = {
        "TEST_API_URL": url,
        MUTATION_OPT_IN: MUTATION_OPT_IN_VALUE,
        RUNTIME_ENV_ATTESTATION: runtime_env,
        DISPOSABLE_DB_NAME: "verdaxis_runtime_test",
    }
    with pytest.raises(RuntimeTestConfigurationError):
        resolve_test_api_url(env, require_mutation_opt_in=True)


def test_mutating_target_accepts_only_known_staging_or_disposable_targets():
    common = {
        MUTATION_OPT_IN: MUTATION_OPT_IN_VALUE,
    }
    assert resolve_test_api_url(
        {**common, "TEST_API_URL": "https://api-staging.verdaxis.exchange", RUNTIME_ENV_ATTESTATION: "staging"},
        require_mutation_opt_in=True,
    ) == "https://api-staging.verdaxis.exchange"
    assert resolve_test_api_url(
        {**common, "TEST_API_URL": "http://127.0.0.1:18765", RUNTIME_ENV_ATTESTATION: "disposable", DISPOSABLE_DB_NAME: "verdaxis_runtime_test"},
        require_mutation_opt_in=True,
    ) == "http://127.0.0.1:18765"


@pytest.mark.parametrize("database_name", ["verdaxis", "verdaxis_staging", "runtime", ""])
def test_disposable_mutation_requires_a_proven_disposable_database(database_name):
    env = {
        "TEST_API_URL": "http://127.0.0.1:18765",
        MUTATION_OPT_IN: MUTATION_OPT_IN_VALUE,
        RUNTIME_ENV_ATTESTATION: "disposable",
        DISPOSABLE_DB_NAME: database_name,
    }
    with pytest.raises(RuntimeTestConfigurationError, match="disposable database"):
        resolve_test_api_url(env, require_mutation_opt_in=True)


def test_systemd_templates_are_loopback_bound_and_sandboxed():
    root = Path(__file__).parents[2]
    for path in (
        root / "deploy/systemd/verdaxis-backend.service",
        root / "deploy/systemd/verdaxis-backend-staging.service",
    ):
        contents = path.read_text()
        assert "--host 127.0.0.1" in contents
        expected_port = "8000" if path.name == "verdaxis-backend.service" else "8001"
        assert f"--port {expected_port}" in contents
        assert "After=network-online.target postgresql.service" in contents
        assert "Requires=postgresql.service" in contents
        assert "docker.service" not in contents
        assert "ExecStartPre=" in contents
        assert "current --check-heads" in contents
        assert "KillSignal=SIGTERM" in contents
        assert "TimeoutStopSec=30" in contents
        assert "NoNewPrivileges=true" in contents
        assert "ProtectSystem=strict" in contents
        assert "ProtectHome=read-only" in contents
        assert "PrivateTmp=true" in contents
        assert "MemoryHigh=768M" in contents
        assert "MemoryMax=1G" in contents
        assert "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6" in contents

        worker_count = contents.split("--workers ", 1)[1].split()[0]
        assert int(worker_count) == 4


def test_migration_verification_is_upgrade_then_drift_check():
    root = Path(__file__).parents[2]
    script = (root / "scripts/verify_migrations.sh").read_text()

    assert "upgrade head" in script
    assert " check" in script
    assert "DATABASE_URL" in script
    assert "expected exactly one standalone Alembic head" in script


def test_migration_drift_check_does_not_suppress_columns_fks_types_defaults_or_comments():
    root = Path(__file__).parents[2]
    source = (root / "alembic/env.py").read_text()

    assert "compare_type=compare_type" in source
    assert "compare_server_default=compare_server_default" in source
    assert "compare_comments=True" in source
    assert "type_ in {\"index\", \"foreign_key_constraint\"}" not in source
    assert "reflected or compare_to is not None" not in source
    drift_source = (root / "app/migration_drift.py").read_text()
    assert "spatial_ref_sys" in drift_source
    assert "LEGACY_TABLES" in drift_source
    assert "reflected" in drift_source


def test_migration_type_comparator_only_normalizes_non_native_enum_storage():
    from sqlalchemy import Enum, Integer, String

    assert compare_type(None, None, None, String(16), Enum("A", native_enum=False, length=16)) is False
    assert compare_type(None, None, None, String(15), Enum("A", native_enum=False, length=16)) is True
    assert compare_type(None, None, None, String(), Enum("A", native_enum=False, length=16)) is True
    assert compare_type(None, None, None, String(16), Integer()) is None


def test_migration_comparison_detects_enum_storage_length_drift():
    from sqlalchemy import Enum

    engine = create_engine("sqlite://")
    actual = MetaData()
    Table("enum_values", actual, Column("status", String(15)))
    actual.create_all(engine)

    expected = MetaData()
    Table("enum_values", expected, Column("status", Enum("OPEN", native_enum=False, length=16)))

    with engine.connect() as connection:
        context = MigrationContext.configure(
            connection,
            opts={
                "target_metadata": expected,
                "compare_type": compare_type,
                "compare_server_default": compare_server_default,
            },
        )
        differences = compare_metadata(context, expected)

    assert any(
        operation[0] == "modify_type"
        for difference_group in differences
        for operation in difference_group
    )


def test_migration_default_comparator_only_normalizes_python_owned_defaults():
    from sqlalchemy import Column, Integer, MetaData, text

    metadata = MetaData()
    python_default = Column("python_default", Integer, default=1)
    explicit_default = Column("explicit_default", Integer, server_default=text("1"))
    assert compare_server_default(None, None, python_default, None, None, None) is False
    assert compare_server_default(None, None, python_default, "1", None, None) is None
    assert compare_server_default(None, None, explicit_default, "1", text("1"), "1") is False
    assert compare_server_default(None, None, explicit_default, "2", text("1"), "1") is None


def test_runtime_migration_retains_delivery_windows_and_required_defaults():
    root = Path(__file__).parents[2]
    migration = (root / "alembic/versions/rh_20260720_runtime_metadata.py").read_text()

    assert 'drop_column("orderbook_orders", "delivery_window_start")' not in migration
    assert 'drop_column("orderbook_orders", "delivery_window_end")' not in migration
    assert "server_default=sa.text(\"'SPOT'\")" in migration or 'server_default="SPOT"' in migration
    assert 'server_default=sa.text("false")' in migration
    assert 'server_default=sa.text("\'OPEN\'")' in migration or 'server_default="OPEN"' in migration
    assert 'server_default=sa.text("now()")' in migration


def test_migration_comparison_detects_an_omitted_foreign_key():
    engine = create_engine("sqlite://")
    actual = MetaData()
    Table("parents", actual, Column("id", Integer, primary_key=True))
    Table("children", actual, Column("id", Integer, primary_key=True), Column("parent_id", Integer))
    actual.create_all(engine)

    expected = MetaData()
    Table("parents", expected, Column("id", Integer, primary_key=True))
    Table(
        "children",
        expected,
        Column("id", Integer, primary_key=True),
        Column("parent_id", Integer, ForeignKey("parents.id")),
    )

    with engine.connect() as connection:
        context = MigrationContext.configure(
            connection,
            opts={
                "target_metadata": expected,
                "include_object": include_object,
                "compare_type": True,
                "compare_server_default": True,
                "compare_comments": True,
            },
        )
        differences = compare_metadata(context, expected)

    assert any(difference[0] == "add_fk" for difference in differences)

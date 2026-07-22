"""Focused tests for runtime configuration and integration safety."""

import os
import importlib.util
from pathlib import Path
import re
import subprocess
import sys
from types import SimpleNamespace

import pytest
from pydantic import ValidationError
from alembic.autogenerate import compare_metadata
from alembic.migration import MigrationContext
from sqlalchemy import Boolean, Column, ForeignKey, Integer, MetaData, String, Table, UniqueConstraint, create_engine
from app.migration_drift import compare_server_default, compare_type, include_object

from app.config import Settings
from app.database import (
    assert_migrator_connection_is_safe,
    assert_database_runtime_is_safe,
    configured_connection_total,
    engine_options,
    migrator_connect_args,
)
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
        "RELEASE_SHA": "test",
    }
    values.update(overrides)
    return Settings(**values)


def _deployed_values(environment: str) -> dict:
    identities = {
        "production": ("verdaxis", "verdaxis_app", "verdaxis_migrator"),
        "staging": (
            "verdaxis_staging",
            "verdaxis_app_staging",
            "verdaxis_migrator_staging",
        ),
    }
    database, app_role, migrator_role = identities[environment]
    return {
        "ENVIRONMENT": environment,
        "RELEASE_SHA": "a" * 40,
        "DATABASE_NAME": database,
        "DATABASE_USER": app_role,
        "DATABASE_PASSWORD": "not-a-default",
        "DATABASE_URL": (
            f"postgresql+asyncpg://{app_role}:not-a-default@localhost/{database}"
        ),
        "MIGRATOR_DATABASE_URL": (
            f"postgresql+asyncpg://{migrator_role}:not-a-default@localhost/{database}"
        ),
        "JWT_SECRET": "x" * 32,
    }


def _role_attestation(database: str, user: str, **overrides) -> dict:
    values = {
        "connected_database": database,
        "connected_user": user,
        "role_can_login": True,
        "role_inherits_privileges": False,
        "connected_role_is_superuser": False,
        "role_can_create_database": False,
        "role_can_create_role": False,
        "role_can_replicate": False,
        "role_bypasses_rls": False,
        "role_has_memberships": False,
    }
    values.update(overrides)
    return values


def test_pool_defaults_leave_headroom_for_four_workers_on_postgres_max_100():
    settings = _settings()

    assert settings.DB_POOL_SIZE == 2
    assert settings.DB_MAX_OVERFLOW == 1
    assert settings.UVICORN_WORKERS == 4
    assert settings.DB_SERVICE_COUNT == 2
    assert settings.DB_MAX_CONNECTIONS == 100
    assert settings.DB_RESERVED_CONNECTIONS == 20
    assert settings.DB_SERVICE_COUNT * settings.UVICORN_WORKERS * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) + settings.DB_RESERVED_CONNECTIONS == 44
    assert settings.DB_SERVICE_COUNT * settings.UVICORN_WORKERS * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) <= (
        settings.DB_MAX_CONNECTIONS - settings.DB_RESERVED_CONNECTIONS
    )
    # The runtime guard also budgets one dedicated SSE dispatcher LISTEN
    # connection per worker: 24 pool + 8 listener + 20 reserve = 52.
    assert configured_connection_total(settings) == 52
    assert configured_connection_total(settings) <= settings.DB_MAX_CONNECTIONS


@pytest.mark.parametrize(
    "unit_name",
    ["verdaxis-backend.service", "verdaxis-backend-staging.service"],
)
def test_deployed_units_use_the_validated_connection_budget(unit_name):
    root = Path(__file__).parents[2]
    service = (root / "deploy/systemd" / unit_name).read_text()
    worker_count = int(service.split("Environment=UVICORN_WORKERS=", 1)[1].splitlines()[0])
    settings = _settings(UVICORN_WORKERS=worker_count)

    assert worker_count == 4
    assert "--workers ${UVICORN_WORKERS}" in service
    assert settings.DB_SERVICE_COUNT * worker_count * (settings.DB_POOL_SIZE + settings.DB_MAX_OVERFLOW) + settings.DB_RESERVED_CONNECTIONS <= settings.DB_MAX_CONNECTIONS


@pytest.mark.parametrize(
    "overrides",
    [
        {"DB_POOL_SIZE": 0},
        {"DB_MAX_OVERFLOW": -1},
        {"UVICORN_WORKERS": 0},
        {"DB_MAX_CONNECTIONS": 10, "DB_RESERVED_CONNECTIONS": 10},
        {"UVICORN_WORKERS": 20, "DB_POOL_SIZE": 5, "DB_MAX_OVERFLOW": 2},
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


def test_production_validates_the_effective_database_url_username(monkeypatch):
    monkeypatch.setenv("ENVIRONMENT", "production")
    with pytest.raises(ValidationError, match="effective DATABASE_URL application role"):
        Settings(
            **{
                **_deployed_values("production"),
                "DATABASE_URL": "postgresql+asyncpg://postgres:not-a-default@localhost/verdaxis",
            }
        )


def test_production_requires_app_and_migrator_on_same_database_endpoint(
    monkeypatch,
):
    monkeypatch.setenv("ENVIRONMENT", "production")
    values = _deployed_values("production")
    values["MIGRATOR_DATABASE_URL"] = (
        "postgresql+asyncpg://verdaxis_migrator:not-a-default@"
        "migration-db/verdaxis"
    )

    with pytest.raises(ValidationError, match="same database endpoint"):
        Settings(**values)
    with pytest.raises(ValidationError, match="effective MIGRATOR_DATABASE_URL migrator role"):
        Settings(
            **{
                **_deployed_values("production"),
                "MIGRATOR_DATABASE_URL": "postgresql+asyncpg://postgres:not-a-default@localhost/verdaxis",
            }
        )


def test_database_startup_identity_and_observed_capacity_are_attested():
    settings = _settings(
        DATABASE_URL="postgresql+asyncpg://verdaxis_app:x@localhost/verdaxis_test"
    )
    assert_database_runtime_is_safe(
        settings,
        observed_max_connections=100,
        **_role_attestation("verdaxis_test", "verdaxis_app"),
    )
    with pytest.raises(RuntimeError, match="does not match"):
        assert_database_runtime_is_safe(
            settings,
            observed_max_connections=100,
            **_role_attestation("verdaxis_test", "unexpected_role"),
        )

    database_source = (Path(__file__).parents[2] / "app/database.py").read_text()
    assert 'text("SHOW max_connections")' in database_source


def test_migrator_connected_role_is_attested():
    settings = _settings(
        DATABASE_URL="postgresql+asyncpg://verdaxis_app:x@localhost/verdaxis_test",
        MIGRATOR_DATABASE_URL="postgresql+asyncpg://verdaxis_migrator:x@localhost/verdaxis_test",
    )
    assert_migrator_connection_is_safe(
        settings,
        **_role_attestation("verdaxis_test", "verdaxis_migrator"),
    )
    with pytest.raises(RuntimeError, match="migrator current_user does not match"):
        assert_migrator_connection_is_safe(
            settings,
            **_role_attestation("verdaxis_test", "verdaxis_app"),
        )
    with pytest.raises(RuntimeError, match="superuser"):
        assert_migrator_connection_is_safe(
            settings,
            **_role_attestation(
                "verdaxis_test",
                "verdaxis_migrator",
                connected_role_is_superuser=True,
            ),
        )
    with pytest.raises(RuntimeError, match="superuser"):
        assert_database_runtime_is_safe(
            settings,
            observed_max_connections=100,
            **_role_attestation(
                "verdaxis_test",
                "verdaxis_app",
                connected_role_is_superuser=True,
            ),
        )
    with pytest.raises(RuntimeError, match="max_connections"):
        assert_database_runtime_is_safe(
            settings,
            observed_max_connections=40,
            **_role_attestation("verdaxis_test", "verdaxis_app"),
        )


def test_app_config_imports_without_ambient_configuration():
    result = subprocess.run(
        [sys.executable, "-c", "import app.config, app.database"],
        cwd=Path(__file__).parents[2],
        env={"PATH": os.environ["PATH"], "PYTHONPATH": "."},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_deployed_environments_require_full_release_sha(environment):
    values = _deployed_values(environment)
    values.pop("RELEASE_SHA")
    with pytest.raises(ValidationError, match="RELEASE_SHA"):
        Settings(**values)
    settings = Settings(**values, RELEASE_SHA="a" * 40)
    assert settings.RELEASE_SHA == "a" * 40


@pytest.mark.parametrize("environment, placeholder", [("development", "development"), ("test", "test")])
def test_non_deployed_environments_allow_only_their_explicit_placeholder(
    environment, placeholder
):
    assert Settings(ENVIRONMENT=environment, RELEASE_SHA=placeholder).RELEASE_SHA == placeholder
    with pytest.raises(ValidationError, match="RELEASE_SHA"):
        Settings(ENVIRONMENT=environment, RELEASE_SHA="unknown")


@pytest.mark.parametrize(
    "environment, expected",
    [
        ("production", {"https://verdaxis.exchange", "https://app.verdaxis.exchange"}),
        ("staging", {"https://staging.verdaxis.exchange"}),
        (
            "development",
            {
                "http://localhost:5173",
                "http://localhost:5174",
                "http://127.0.0.1:5173",
                "http://127.0.0.1:5174",
            },
        ),
    ],
)
def test_cors_defaults_are_environment_specific(monkeypatch, environment, expected):
    monkeypatch.setenv("ENVIRONMENT", environment)
    values = (
        _deployed_values(environment)
        if environment in {"production", "staging"}
        else {"ENVIRONMENT": environment, "RELEASE_SHA": environment}
    )
    settings = Settings(**values)
    assert set(settings.BACKEND_CORS_ORIGINS) == expected


@pytest.mark.parametrize(
    "environment, origin",
    [
        ("production", "https://staging.verdaxis.exchange"),
        ("production", "http://localhost:5173"),
        ("staging", "https://app.verdaxis.exchange"),
        ("staging", "http://127.0.0.1:5173"),
        ("development", "https://app.verdaxis.exchange"),
        ("development", "*"),
        ("development", "https://*.verdaxis.exchange"),
        ("development", "http://user:pass@localhost:5173"),
        ("development", "http://localhost:5173/path"),
    ],
)
def test_credentialed_cors_rejects_cross_environment_or_non_origin_values(
    monkeypatch, environment, origin
):
    monkeypatch.setenv("ENVIRONMENT", environment)
    values = (
        _deployed_values(environment)
        if environment in {"production", "staging"}
        else {"ENVIRONMENT": environment, "RELEASE_SHA": environment}
    )
    values["BACKEND_CORS_ORIGINS"] = [origin]
    with pytest.raises(ValidationError, match="CORS"):
        Settings(**values)


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
    assert "verify_migrator_connection(connection, settings)" in source


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
        {**common, "TEST_API_URL": "http://127.0.0.1:8001", RUNTIME_ENV_ATTESTATION: "staging"},
        require_mutation_opt_in=True,
    ) == "http://127.0.0.1:8001"
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
        assert ".runtime-release.env" in contents
        assert "current --check-heads" not in contents
        assert "scripts/verify_migration_revision.py" in contents
        assert "--expected ${MIGRATION_REVISION}" in contents
        assert "KillSignal=SIGTERM" in contents
        assert "TimeoutStopSec=30" in contents
        assert "NoNewPrivileges=true" in contents
        assert "ProtectSystem=strict" in contents
        assert "ProtectHome=read-only" in contents
        assert "PrivateTmp=true" in contents
        assert "MemoryHigh=768M" in contents
        assert "MemoryMax=1G" in contents
        assert "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6" in contents

        worker_count = contents.split("Environment=UVICORN_WORKERS=", 1)[1].splitlines()[0]
        assert int(worker_count) == 4


def test_deploy_script_hands_release_sha_to_systemd_atomically():
    script = (Path(__file__).parents[2] / "scripts/deploy.sh").read_text()
    assert "RELEASE_SHA=" in script
    assert "MIGRATION_REVISION=" in script
    assert ".runtime-release.env" in script
    assert "mv --" in script
    assert script.index("mv --") < script.index("systemctl restart")


def test_migration_verification_is_upgrade_then_drift_check():
    root = Path(__file__).parents[2]
    script = (root / "scripts/verify_migrations.sh").read_text()

    assert "upgrade head" in script
    assert " check" in script
    assert "DATABASE_URL" in script
    assert "expected exactly one standalone Alembic head" in script


def test_alembic_closes_identity_attestation_transaction_before_migrations():
    source = (Path(__file__).parents[2] / "alembic/env.py").read_text()
    verification = source.index("verify_migrator_connection(connection, settings)")
    attestation_commit = source.index("connection.commit()", verification)
    configure = source.index("context.configure(", verification)
    assert verification < attestation_commit < configure


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


def test_postgis_and_external_object_exclusions_require_exact_schema_table_and_shape():
    public_metadata = MetaData(schema="public")
    public_state = Table("state", public_metadata, Column("id", Integer))
    assert include_object(public_state, "state", "table", True, None)

    implicit_topology = Table(
        "topology",
        MetaData(),
        Column("id", Integer),
        Column("name", String),
        Column("srid", Integer),
        Column("precision", Integer),
        Column("hasz", Boolean),
        Column("useslargeids", Boolean),
    )
    assert not include_object(implicit_topology, "topology", "table", True, None)
    unrelated_topology = Table(
        "topology", MetaData(), Column("id", Integer), Column("owner_id", Integer)
    )
    assert include_object(unrelated_topology, "topology", "table", True, None)

    tiger_metadata = MetaData(schema="tiger")
    tiger_state = Table("state", tiger_metadata, Column("gid", Integer))
    assert not include_object(tiger_state, "state", "table", True, None)

    news = Table(
        "news_items",
        public_metadata,
        Column("reference_number", String(20)),
        UniqueConstraint("reference_number", name="reference_number_key"),
        extend_existing=True,
    )
    news_constraint = next(
        constraint for constraint in news.constraints
        if constraint.name == "reference_number_key"
    )
    assert include_object(
        news_constraint, "reference_number_key", "unique_constraint", True, None
    )

    rfqs = Table(
        "rfqs",
        public_metadata,
        Column("reference_number", String(20)),
        UniqueConstraint("reference_number", name="reference_number_key"),
        extend_existing=True,
    )
    rfq_constraint = next(
        constraint for constraint in rfqs.constraints
        if constraint.name == "reference_number_key"
    )
    assert not include_object(
        rfq_constraint, "reference_number_key", "unique_constraint", True, None
    )


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
    from sqlalchemy import Column, Integer, text

    python_default = Column("python_default", Integer, default=1)
    explicit_default = Column("explicit_default", Integer, server_default=text("1"))
    assert compare_server_default(None, None, python_default, None, None, None) is False
    assert compare_server_default(None, None, python_default, "1", None, None) is None
    assert compare_server_default(None, None, explicit_default, "1", text("1"), "1") is False
    assert compare_server_default(None, None, explicit_default, "2", text("1"), "1") is None


def test_changed_python_default_is_not_hidden_by_a_historical_server_default():
    metadata = MetaData(schema="public")
    unchanged = Table(
        "news_items", metadata, Column("category", String, default="markets")
    ).c.category
    changed = Table(
        "news_items_changed", MetaData(schema="public"),
        Column("category", String, default="shipping"),
    ).c.category
    changed.table.name = "news_items"

    assert compare_server_default(None, None, unchanged, "'markets'", None, None) is False
    assert compare_server_default(None, None, changed, "'markets'", None, None) is None


def test_runtime_migration_retains_delivery_windows_and_required_defaults():
    root = Path(__file__).parents[2]
    migration = (root / "alembic/versions/rh_20260720_runtime_metadata.py").read_text()

    assert 'drop_column("orderbook_orders", "delivery_window_start")' not in migration
    assert 'drop_column("orderbook_orders", "delivery_window_end")' not in migration
    assert "server_default=sa.text(\"'SPOT'\")" in migration or 'server_default="SPOT"' in migration
    assert 'server_default=sa.text("false")' in migration
    assert 'server_default=sa.text("\'OPEN\'")' in migration or 'server_default="OPEN"' in migration
    assert 'server_default=sa.text("now()")' in migration
    assert "cannot narrow inventory_items.fuel_type" in migration
    assert "length(fuel_type) > 8" in migration


def test_runtime_downgrade_refuses_biomethane_with_actionable_error(monkeypatch):
    path = Path(__file__).parents[2] / "alembic/versions/rh_20260720_runtime_metadata.py"
    spec = importlib.util.spec_from_file_location("runtime_metadata_migration", path)
    migration = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(migration)

    bind = SimpleNamespace(scalar=lambda statement: "Biomethane")
    monkeypatch.setattr(migration.op, "get_bind", lambda: bind)
    with pytest.raises(RuntimeError, match="Biomethane requires the widened schema"):
        migration._assert_inventory_fuel_type_fits_legacy_width()


def test_runtime_migration_and_model_remove_only_the_proven_redundant_indexes():
    root = Path(__file__).parents[2]
    migration = (root / "alembic/versions/rh_20260720_runtime_metadata.py").read_text()
    model = (root / "app/models/orderbook.py").read_text()
    redundant = {
        "ix_orderbook_orders_side",
        "ix_orderbook_orders_status",
        "ix_orderbook_orders_org",
    }
    for name in redundant:
        assert f'op.drop_index("{name}", table_name="orderbook_orders")' in migration
        assert f'Index("{name}"' not in model
    assert "ix_orderbook_orders_active_slice_lookup" in model


def test_postgres_helper_is_digest_pinned_and_checks_numeric_versions():
    script = (Path(__file__).parents[2] / "scripts/run_product_analytics_postgres_tests.sh").read_text()
    assert "postgis/postgis:17-3.6-alpine@sha256:" in script
    assert "server_version_num" in script
    assert "PostGIS_Lib_Version()" in script
    assert 'POSTGRES_VERSION_NUM" -lt 170000' in script
    assert 'POSTGRES_VERSION_NUM" -ge 180000' in script


def test_least_privilege_role_artifacts_cover_existing_and_future_objects():
    root = Path(__file__).parents[2]
    bootstrap = (root / "deploy/postgres/bootstrap_roles.sql").read_text()
    validation = (root / "deploy/postgres/validate_roles.sql").read_text()
    policy_path = root / "deploy/postgres/app_acl_policy.sql"
    convergence_path = root / "deploy/postgres/converge_runtime_object_acls.sql"

    assert policy_path.exists()
    assert convergence_path.exists()
    policy = policy_path.read_text()
    convergence = convergence_path.read_text()

    for role in ("app_role", "migrator_role", "backup_role"):
        assert role in bootstrap
        assert role in validation
    assert "NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION" in bootstrap
    assert "app_table_policy" in convergence
    assert "app_column_policy" in convergence
    assert "app_table_policy" in validation
    assert "app_column_policy" in validation
    assert "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES" not in bootstrap
    assert "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE" not in bootstrap
    assert "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO" not in bootstrap
    assert "alembic_version" in bootstrap
    assert "spatial_ref_sys" in bootstrap
    assert "pg_auth_members" in bootstrap
    assert "pg_attribute" in convergence
    assert "attacl" in convergence
    assert "REVOKE ALL PRIVILEGES (%I) ON TABLE" in convergence
    assert "CASCADE" in bootstrap
    assert "ORDER BY" in bootstrap
    assert "ALTER DEFAULT PRIVILEGES FOR ROLE" in bootstrap
    assert "statement_timeout" in bootstrap
    assert "idle_in_transaction_session_timeout" in bootstrap
    assert "has_table_privilege" in validation
    assert "has_sequence_privilege" in validation
    assert "acldefault" in validation
    assert "pg_attribute" in validation
    assert "attacl" in validation
    assert "has_column_privilege" in validation
    for protected_table in (
        "alembic_version",
        "seed_runs",
        "market_row_quarantines",
        "organization_market_approvals",
        "market_signal_ingestion_runs",
    ):
        assert (
            protected_table in policy
            or protected_table in bootstrap
            or protected_table in validation
        )
    assert "('audit_logs', ARRAY['SELECT', 'INSERT'])" in policy
    assert "('user_status_transitions', ARRAY['SELECT', 'INSERT'])" in policy
    column_policy = policy.split("app_column_policy", 1)[-1]
    # The security checkpoint's admin organization admission review is the
    # single app-role write path for verification_status: UPDATE only.
    assert "('organizations', 'verification_status', 'INSERT')" not in column_policy
    assert "('organizations', 'verification_status', 'UPDATE')" in column_policy
    assert "provenance" not in column_policy

    assert "\\ir app_acl_policy.sql" in convergence
    assert "\\ir converge_runtime_object_acls.sql" in bootstrap
    for shared_statement in (
        "CREATE TEMP TABLE governed_objects",
        "REVOKE ALL PRIVILEGES (%I) ON TABLE",
        "JOIN app_column_policy AS policy",
        "JOIN app_sequence_policy AS policy",
    ):
        assert shared_statement in convergence
        assert shared_statement not in bootstrap


def test_postgres_runner_uses_the_selected_venv_for_migration_tools():
    source = (
        Path(__file__).parents[2]
        / "scripts/run_product_analytics_postgres_tests.sh"
    ).read_text()

    path_setup = 'export PATH="$(dirname "$PYTEST_BIN"):$PATH"'
    assert path_setup in source
    assert source.index(path_setup) < source.index("./scripts/verify_migrations.sh")


def test_organization_registration_columns_match_existing_write_paths():
    root = Path(__file__).parents[2]
    model_source = (root / "app/models/user.py").read_text()
    registration_source = (root / "app/routers/auth_simple.py").read_text()
    policy = (root / "deploy/postgres/app_acl_policy.sql").read_text()

    organization_source = model_source.split("class Organization", 1)[1].split(
        "class User", 1
    )[0]
    assert 'server_default="PENDING"' in organization_source
    assert 'default="PENDING"' not in organization_source.replace(
        'server_default="PENDING"', ""
    )
    registration_block = registration_source.split(
        "async def register_with_org", 1
    )[1].split("db.add(new_org)", 1)[0]
    assert 'verification_status="PENDING"' not in registration_block
    assert "('organizations', 'verification_status', 'INSERT')" not in policy
    # The security checkpoint added exactly one app-role write path for
    # verification_status: the admin organization admission review endpoints.
    # UPDATE is therefore declared; INSERT stays denied (server_default only).
    assert "('organizations', 'verification_status', 'UPDATE')" in policy

    from sqlalchemy.dialects import postgresql
    from app.models.user import Organization, OrgType

    insert_sql = str(
        Organization.__table__.insert()
        .values(name="New Org", domain="new.invalid", type=OrgType.FUEL_BUYER)
        .compile(dialect=postgresql.dialect())
    )
    assert "verification_status" not in insert_sql


def test_organization_column_acl_is_the_exact_reviewed_set():
    policy = (
        Path(__file__).parents[2] / "deploy/postgres/app_acl_policy.sql"
    ).read_text()
    entries = {
        (column_name, privilege)
        for column_name, privilege in re.findall(
            r"\('organizations', '([^']+)', '(INSERT|UPDATE)'\)", policy
        )
    }

    assert entries == {
        ("id", "INSERT"),
        ("name", "INSERT"),
        ("domain", "INSERT"),
        ("type", "INSERT"),
        ("supplier_tier", "INSERT"),
        ("tax_id", "INSERT"),
        ("country_code", "INSERT"),
        ("created_at", "INSERT"),
        ("name", "UPDATE"),
        ("domain", "UPDATE"),
        ("type", "UPDATE"),
        ("supplier_tier", "UPDATE"),
        ("tax_id", "UPDATE"),
        ("country_code", "UPDATE"),
        ("verification_status", "UPDATE"),
    }


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

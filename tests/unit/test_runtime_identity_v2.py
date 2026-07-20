"""Exact deployed database/runtime identity regression tests."""

from pathlib import Path

import pytest
from pydantic import ValidationError

from app.config import Settings
from app.database import (
    assert_database_runtime_is_safe,
    assert_migrator_connection_is_safe,
)


def _deployed_settings(environment: str, **overrides) -> Settings:
    identities = {
        "production": (
            "verdaxis",
            "verdaxis_app",
            "verdaxis_migrator",
        ),
        "staging": (
            "verdaxis_staging",
            "verdaxis_app_staging",
            "verdaxis_migrator_staging",
        ),
    }
    database, app_role, migrator_role = identities[environment]
    values = {
        "ENVIRONMENT": environment,
        "RELEASE_SHA": "a" * 40,
        "JWT_SECRET": "x" * 32,
        "DATABASE_NAME": database,
        "DATABASE_USER": app_role,
        "DATABASE_PASSWORD": "runtime-test-only",
        "DATABASE_URL": (
            f"postgresql+asyncpg://{app_role}:runtime-test-only@localhost/{database}"
        ),
        "MIGRATOR_DATABASE_URL": (
            f"postgresql+asyncpg://{migrator_role}:runtime-test-only@localhost/{database}"
        ),
    }
    values.update(overrides)
    return Settings(**values)


def _safe_role(database: str, user: str) -> dict:
    return {
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


@pytest.mark.parametrize(
    "environment,database,app_role,migrator_role",
    [
        ("production", "verdaxis", "verdaxis_app", "verdaxis_migrator"),
        (
            "staging",
            "verdaxis_staging",
            "verdaxis_app_staging",
            "verdaxis_migrator_staging",
        ),
    ],
)
def test_deployed_identity_is_exact(environment, database, app_role, migrator_role):
    settings = _deployed_settings(environment)

    assert settings.DATABASE_NAME == database
    assert settings.DATABASE_USER == app_role
    assert settings.DATABASE_URL.endswith(f"/{database}")
    assert settings.MIGRATOR_DATABASE_URL.endswith(f"/{database}")
    assert app_role in settings.DATABASE_URL
    assert migrator_role in settings.MIGRATOR_DATABASE_URL


@pytest.mark.parametrize(
    "environment,overrides,error",
    [
        (
            "production",
            {
                "DATABASE_NAME": "verdaxis_staging",
                "DATABASE_URL": "postgresql+asyncpg://verdaxis_app:x@localhost/verdaxis_staging",
                "MIGRATOR_DATABASE_URL": "postgresql+asyncpg://verdaxis_migrator:x@localhost/verdaxis_staging",
            },
            "database",
        ),
        (
            "production",
            {
                "DATABASE_URL": "postgresql+asyncpg://verdaxis_migrator:x@localhost/verdaxis_staging",
            },
            "database",
        ),
        (
            "production",
            {
                "MIGRATOR_DATABASE_URL": "postgresql+asyncpg://verdaxis_migrator:x@localhost/verdaxis_staging",
            },
            "MIGRATOR_DATABASE_URL database",
        ),
        (
            "staging",
            {
                "DATABASE_USER": "verdaxis_app",
                "DATABASE_URL": "postgresql+asyncpg://verdaxis_app:x@localhost/verdaxis_staging",
            },
            "application role",
        ),
        (
            "staging",
            {
                "MIGRATOR_DATABASE_URL": "postgresql+asyncpg://verdaxis_app_staging:x@localhost/verdaxis_staging",
            },
            "migrator role",
        ),
    ],
)
def test_deployed_identity_rejects_cross_environment_or_same_role(
    environment, overrides, error
):
    with pytest.raises(ValidationError, match=error):
        _deployed_settings(environment, **overrides)


@pytest.mark.parametrize("environment", ["production", "staging"])
def test_deployed_environment_rejects_sqlite_default_jwt_and_auth_bypass(environment):
    with pytest.raises(ValidationError, match="PostgreSQL"):
        _deployed_settings(
            environment,
            DATABASE_URL="sqlite+aiosqlite:///:memory:",
        )
    with pytest.raises(ValidationError, match="JWT_SECRET"):
        _deployed_settings(environment, JWT_SECRET="change-me-in-production")
    with pytest.raises(ValidationError, match="ENABLE_AUTH_BYPASS"):
        _deployed_settings(environment, ENABLE_AUTH_BYPASS=True)


@pytest.mark.parametrize("environment", ["production", "staging"])
@pytest.mark.parametrize("url_field", ["DATABASE_URL", "MIGRATOR_DATABASE_URL"])
@pytest.mark.parametrize("placeholder", ["postgres", "change_me"])
def test_deployed_app_and_migrator_reject_the_same_placeholder_passwords_without_echo(
    environment, url_field, placeholder
):
    settings = _deployed_settings(environment)
    original_url = getattr(settings, url_field)
    unsafe_url = original_url.replace("runtime-test-only", placeholder)

    with pytest.raises(ValidationError, match=f"{url_field} password") as exc_info:
        _deployed_settings(environment, **{url_field: unsafe_url})

    rendered_error = str(exc_info.value)
    assert unsafe_url not in rendered_error
    assert f":{placeholder}@" not in rendered_error
    assert placeholder not in rendered_error


@pytest.mark.parametrize("environment", ["production", "staging"])
@pytest.mark.parametrize("url_field", ["DATABASE_URL", "MIGRATOR_DATABASE_URL"])
@pytest.mark.parametrize("encoded_password", ["", "%20", "%20%09%20"])
def test_deployed_app_and_migrator_reject_blank_decoded_passwords_without_echo(
    environment, url_field, encoded_password
):
    settings = _deployed_settings(environment)
    original_url = getattr(settings, url_field)
    unsafe_url = original_url.replace("runtime-test-only", encoded_password)

    with pytest.raises(ValidationError, match=f"{url_field} password") as exc_info:
        _deployed_settings(environment, **{url_field: unsafe_url})

    rendered_error = str(exc_info.value)
    assert unsafe_url not in rendered_error
    assert f":{encoded_password}@" not in rendered_error


@pytest.mark.parametrize("environment", ["production", "staging"])
def test_deployed_topology_cannot_undercount_services_or_reserve(environment):
    with pytest.raises(ValidationError, match="DB_SERVICE_COUNT"):
        _deployed_settings(environment, DB_SERVICE_COUNT=1)
    with pytest.raises(ValidationError, match="DB_RESERVED_CONNECTIONS"):
        _deployed_settings(environment, DB_RESERVED_CONNECTIONS=19)


@pytest.mark.parametrize(
    "query",
    ["host=/tmp", "database=verdaxis_staging", "target_session_attrs=read-write"],
)
def test_migration_urls_reject_all_connection_routing_query_parameters(query):
    with pytest.raises(ValidationError, match="query parameters"):
        _deployed_settings(
            "production",
            MIGRATOR_DATABASE_URL=(
                "postgresql+asyncpg://verdaxis_migrator:x@localhost/verdaxis?" + query
            ),
        )


def test_runtime_and_migrator_attest_database_user_role_properties_and_memberships():
    settings = _deployed_settings("production")
    assert_database_runtime_is_safe(
        settings,
        observed_max_connections=100,
        **_safe_role("verdaxis", "verdaxis_app"),
    )
    assert_migrator_connection_is_safe(
        settings,
        **_safe_role("verdaxis", "verdaxis_migrator"),
    )

    for mutation, error in (
        ({"connected_database": "verdaxis_staging"}, "current_database"),
        ({"role_can_create_database": True}, "least-privilege"),
        ({"role_can_create_role": True}, "least-privilege"),
        ({"role_can_replicate": True}, "least-privilege"),
        ({"role_bypasses_rls": True}, "least-privilege"),
        ({"role_inherits_privileges": True}, "least-privilege"),
        ({"role_has_memberships": True}, "membership"),
    ):
        role = _safe_role("verdaxis", "verdaxis_app")
        role.update(mutation)
        with pytest.raises(RuntimeError, match=error):
            assert_database_runtime_is_safe(
                settings,
                observed_max_connections=100,
                **role,
            )


def test_sqlalchemy_engines_hide_parameters_and_migrations_attest_database():
    root = Path(__file__).parents[2]
    engine_sources = (
        root / "app/database.py",
        root / "app/seeds/safety.py",
        root / "alembic/env.py",
        root / "scripts/explain_product_analytics.py",
        root / "scripts/prune_product_analytics.py",
    )

    for source in engine_sources:
        assert "hide_parameters=True" in source.read_text()
    assert "current_database()" in (root / "app/database.py").read_text()


def test_runtime_has_no_per_worker_news_scheduler_or_removed_legacy_routes():
    main_source = (Path(__file__).parents[2] / "app/main.py").read_text()

    assert "_news_refresh_loop" not in main_source
    assert "refresh_news" not in main_source
    assert "asyncio.create_task" not in main_source
    assert "compliance_router" not in main_source
    assert "routers import dashboard" not in main_source


def test_runtime_migration_contains_no_security_or_market_feature_ddl():
    migration = (
        Path(__file__).parents[2]
        / "alembic/versions/rh_20260720_runtime_metadata.py"
    ).read_text()

    assert "create_table" not in migration
    assert "drop_table" not in migration
    assert "kyc" not in migration.lower()
    assert "security" not in migration.lower()

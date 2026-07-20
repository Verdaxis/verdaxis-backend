"""Disposable PostgreSQL proofs for exact runtime role authority."""

from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess

import pytest
from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine


ROOT = Path(__file__).parents[2]
ROLE_NAME = re.compile(r"^[a-z][a-z0-9_]*$")


def _policy_values() -> dict[str, str]:
    values = {
        "database_name": os.environ.get("RUNTIME_TEST_DATABASE_NAME", ""),
        "app_role": os.environ.get("RUNTIME_TEST_APP_ROLE", ""),
        "migrator_role": os.environ.get("RUNTIME_TEST_MIGRATOR_ROLE", ""),
        "backup_role": os.environ.get("RUNTIME_TEST_BACKUP_ROLE", ""),
    }
    if not values["database_name"].endswith("_analytics_test"):
        pytest.skip("disposable runtime role-policy environment is not configured")
    if not all(ROLE_NAME.fullmatch(value) for value in values.values()):
        raise RuntimeError("runtime role-policy identifiers are invalid")
    return values


def _psql(script_name: str) -> subprocess.CompletedProcess[str]:
    raw_url = os.environ.get("POSTGRES_ADMIN_TEST_DATABASE_URL", "")
    if not raw_url:
        pytest.skip("POSTGRES_ADMIN_TEST_DATABASE_URL is not configured")
    url = make_url(raw_url)
    values = _policy_values()
    command = [
        "psql",
        "-X",
        "-h",
        url.host or "",
        "-p",
        str(url.port or 5432),
        "-U",
        url.username or "",
        "-d",
        url.database or "",
    ]
    for key, value in values.items():
        command.extend(("-v", f"{key}={value}"))
    command.extend(("-f", str(ROOT / "deploy/postgres" / script_name)))
    return subprocess.run(
        command,
        env={**os.environ, "PGPASSWORD": url.password or ""},
        capture_output=True,
        text=True,
        check=False,
    )


async def _execute_admin(statement: str) -> None:
    raw_url = os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"]
    engine = create_async_engine(raw_url, hide_parameters=True)
    try:
        async with engine.begin() as connection:
            await connection.execute(text(statement))
    finally:
        await engine.dispose()


async def _fetch_admin(statement: str) -> tuple:
    raw_url = os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"]
    engine = create_async_engine(raw_url, hide_parameters=True)
    try:
        async with engine.connect() as connection:
            return tuple((await connection.execute(text(statement))).one())
    finally:
        await engine.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize("stale_policy", ["membership", "backup_insert", "default_insert"])
async def test_validation_rejects_stale_escalation_and_insert_authority(stale_policy):
    values = _policy_values()
    app = values["app_role"]
    migrator = values["migrator_role"]
    backup = values["backup_role"]
    escalation = "verdaxis_escalation_parent_test"

    if stale_policy == "membership":
        await _execute_admin(f"DROP ROLE IF EXISTS {escalation}")
        await _execute_admin(f"CREATE ROLE {escalation} SUPERUSER NOLOGIN")
        await _execute_admin(f"GRANT {escalation} TO {app}")
    elif stale_policy == "backup_insert":
        await _execute_admin(f"GRANT INSERT ON TABLE public.organizations TO {backup}")
    else:
        await _execute_admin(
            f"ALTER DEFAULT PRIVILEGES FOR ROLE {migrator} IN SCHEMA public "
            f"GRANT INSERT ON TABLES TO {backup}"
        )

    try:
        rejected = _psql("validate_roles.sql")
        assert rejected.returncode != 0

        repaired = _psql("bootstrap_roles.sql")
        assert repaired.returncode == 0, repaired.stderr
        accepted = _psql("validate_roles.sql")
        assert accepted.returncode == 0, accepted.stderr
    finally:
        _psql("bootstrap_roles.sql")
        if stale_policy == "membership":
            await _execute_admin(f"DROP ROLE IF EXISTS {escalation}")


@pytest.mark.asyncio
async def test_unexpected_database_and_schema_grantee_is_rejected_then_revoked():
    values = _policy_values()
    database = values["database_name"]
    unexpected = "verdaxis_unexpected_acl_test"
    await _execute_admin(f"DROP ROLE IF EXISTS {unexpected}")
    await _execute_admin(f"CREATE ROLE {unexpected} NOLOGIN")
    await _execute_admin(
        f"GRANT CREATE, TEMPORARY ON DATABASE {database} TO {unexpected}"
    )
    await _execute_admin(f"GRANT CREATE ON SCHEMA public TO {unexpected}")

    try:
        rejected = _psql("validate_roles.sql")
        assert rejected.returncode != 0

        repaired = _psql("bootstrap_roles.sql")
        assert repaired.returncode == 0, repaired.stderr
        accepted = _psql("validate_roles.sql")
        assert accepted.returncode == 0, accepted.stderr

        database_authority, schema_authority = await _fetch_admin(
            "SELECT "
            f"has_database_privilege('{unexpected}', current_database(), 'CREATE,TEMPORARY'), "
            f"has_schema_privilege('{unexpected}', 'public', 'CREATE')"
        )
        assert database_authority is False
        assert schema_authority is False
    finally:
        await _execute_admin(f"DROP OWNED BY {unexpected}")
        await _execute_admin(f"DROP ROLE IF EXISTS {unexpected}")


@pytest.mark.asyncio
async def test_app_and_backup_cannot_mutate_control_or_extension_objects():
    app_url = os.environ["DATABASE_URL"]
    backup_url = os.environ["BACKUP_DATABASE_URL"]

    for url, statement in (
        (app_url, "UPDATE public.alembic_version SET version_num = version_num"),
        (
            app_url,
            "UPDATE public.spatial_ref_sys SET auth_name = auth_name WHERE srid = 4326",
        ),
        (backup_url, "INSERT INTO public.organizations DEFAULT VALUES"),
    ):
        engine = create_async_engine(url, hide_parameters=True)
        try:
            with pytest.raises(DBAPIError):
                async with engine.begin() as connection:
                    await connection.execute(text(statement))
        finally:
            await engine.dispose()


@pytest.mark.asyncio
async def test_database_schema_and_app_objects_are_owned_by_migrator():
    values = _policy_values()
    admin_url = os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"]
    engine = create_async_engine(admin_url, hide_parameters=True)
    try:
        async with engine.connect() as connection:
            database_owner = (
                await connection.execute(
                    text(
                        "SELECT owner.rolname FROM pg_catalog.pg_database AS database "
                        "JOIN pg_catalog.pg_roles AS owner ON owner.oid = database.datdba "
                        "WHERE database.datname = current_database()"
                    )
                )
            ).scalar_one()
            schema_owner = (
                await connection.execute(
                    text(
                        "SELECT owner.rolname FROM pg_catalog.pg_namespace AS namespace "
                        "JOIN pg_catalog.pg_roles AS owner ON owner.oid = namespace.nspowner "
                        "WHERE namespace.nspname = 'public'"
                    )
                )
            ).scalar_one()
            wrong_object_owners = (
                await connection.execute(
                    text(
                        "SELECT count(*) FROM pg_catalog.pg_class AS object "
                        "JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace "
                        "JOIN pg_catalog.pg_roles AS owner ON owner.oid = object.relowner "
                        "WHERE namespace.nspname = 'public' "
                        "AND object.relkind IN ('r', 'p', 'S') "
                        "AND object.relname NOT IN ('alembic_version', 'spatial_ref_sys') "
                        "AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend AS dependency "
                        "WHERE dependency.classid = 'pg_class'::regclass "
                        "AND dependency.objid = object.oid AND dependency.deptype = 'e') "
                        "AND owner.rolname <> :migrator"
                    ),
                    {"migrator": values["migrator_role"]},
                )
            ).scalar_one()
    finally:
        await engine.dispose()

    assert database_owner == values["migrator_role"]
    assert schema_owner == values["migrator_role"]
    assert wrong_object_owners == 0

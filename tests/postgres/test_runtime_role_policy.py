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


async def _fetchall_admin(statement: str) -> set[tuple]:
    raw_url = os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"]
    engine = create_async_engine(raw_url, hide_parameters=True)
    try:
        async with engine.connect() as connection:
            return {
                tuple(row) for row in (await connection.execute(text(statement))).all()
            }
    finally:
        await engine.dispose()


async def _execute_admin_as(role: str, statements: list[str]) -> None:
    if not ROLE_NAME.fullmatch(role):
        raise RuntimeError("runtime role-policy identifier is invalid")
    raw_url = os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"]
    engine = create_async_engine(raw_url, hide_parameters=True)
    try:
        async with engine.begin() as connection:
            await connection.execute(text(f"SET ROLE {role}"))
            for statement in statements:
                await connection.execute(text(statement))
            await connection.execute(text("RESET ROLE"))
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
async def test_delegated_database_and_schema_grants_are_cascade_revoked():
    values = _policy_values()
    database = values["database_name"]
    parent = "verdaxis_unexpected_acl_test"
    child = "verdaxis_delegated_acl_test"
    membership_parent = "verdaxis_membership_parent_test"
    membership_child = "verdaxis_membership_child_test"
    await _execute_admin(f"DROP ROLE IF EXISTS {child}")
    await _execute_admin(f"DROP ROLE IF EXISTS {parent}")
    await _execute_admin(f"DROP ROLE IF EXISTS {membership_child}")
    await _execute_admin(f"DROP ROLE IF EXISTS {membership_parent}")
    await _execute_admin(f"CREATE ROLE {parent} NOLOGIN")
    await _execute_admin(f"CREATE ROLE {child} NOLOGIN")
    await _execute_admin(f"CREATE ROLE {membership_parent} NOLOGIN")
    await _execute_admin(f"CREATE ROLE {membership_child} NOLOGIN")
    await _execute_admin(
        f"GRANT CREATE, TEMPORARY ON DATABASE {database} TO {parent} "
        "WITH GRANT OPTION"
    )
    await _execute_admin(
        f"GRANT CREATE ON SCHEMA public TO {parent} WITH GRANT OPTION"
    )
    await _execute_admin_as(
        parent,
        [
            f"GRANT CREATE, TEMPORARY ON DATABASE {database} TO {child}",
            f"GRANT CREATE ON SCHEMA public TO {child}",
        ],
    )
    await _execute_admin(
        f"GRANT {membership_parent} TO {values['app_role']} WITH ADMIN OPTION"
    )
    await _execute_admin(
        f"GRANT {membership_child} TO {values['app_role']} WITH ADMIN OPTION"
    )
    await _execute_admin_as(
        values["app_role"],
        [f"GRANT {membership_child} TO {membership_parent}"],
    )

    try:
        rejected = _psql("validate_roles.sql")
        assert rejected.returncode != 0

        repaired = _psql("bootstrap_roles.sql")
        assert repaired.returncode == 0, repaired.stderr
        accepted = _psql("validate_roles.sql")
        assert accepted.returncode == 0, accepted.stderr

        for role in (parent, child):
            database_authority, schema_authority = await _fetch_admin(
                "SELECT "
                f"has_database_privilege('{role}', current_database(), 'CREATE,TEMPORARY'), "
                f"has_schema_privilege('{role}', 'public', 'CREATE')"
            )
            assert database_authority is False
            assert schema_authority is False
        remaining_memberships = await _fetchall_admin(
            "SELECT granted.rolname, member.rolname "
            "FROM pg_catalog.pg_auth_members AS membership "
            "JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid "
            "JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member "
            f"WHERE granted.rolname IN ('{membership_parent}', '{membership_child}') "
            f"OR member.rolname IN ('{membership_parent}', '{membership_child}')"
        )
        assert remaining_memberships == set()
    finally:
        _psql("bootstrap_roles.sql")
        await _execute_admin(f"DROP OWNED BY {child}")
        await _execute_admin(f"DROP OWNED BY {parent}")
        await _execute_admin(f"DROP ROLE IF EXISTS {child}")
        await _execute_admin(f"DROP ROLE IF EXISTS {parent}")
        await _execute_admin(f"DROP ROLE IF EXISTS {membership_child}")
        await _execute_admin(f"DROP ROLE IF EXISTS {membership_parent}")


@pytest.mark.asyncio
async def test_governed_object_acls_are_exactly_repaired_with_cascade():
    values = _policy_values()
    migrator = values["migrator_role"]
    parent_role = "verdaxis_object_grant_parent_test"
    child_role = "verdaxis_object_grant_child_test"
    parent_table = "runtime_acl_parent_test"
    child_table = "runtime_acl_partition_test"
    sequence = "runtime_acl_sequence_test"

    await _execute_admin(f"DROP TABLE IF EXISTS public.{parent_table} CASCADE")
    await _execute_admin(f"DROP SEQUENCE IF EXISTS public.{sequence}")
    await _execute_admin(f"DROP ROLE IF EXISTS {child_role}")
    await _execute_admin(f"DROP ROLE IF EXISTS {parent_role}")
    await _execute_admin(f"CREATE ROLE {parent_role} NOLOGIN")
    await _execute_admin(f"CREATE ROLE {child_role} NOLOGIN")
    await _execute_admin_as(
        migrator,
        [
            f"CREATE TABLE public.{parent_table} (id bigint, bucket integer) "
            "PARTITION BY RANGE (bucket)",
            f"CREATE TABLE public.{child_table} PARTITION OF "
            f"public.{parent_table} FOR VALUES FROM (0) TO (10)",
            f"CREATE SEQUENCE public.{sequence}",
        ],
    )

    for object_name in (parent_table, child_table):
        await _execute_admin(
            f"GRANT SELECT ON TABLE public.{object_name} TO {parent_role} "
            "WITH GRANT OPTION"
        )
        await _execute_admin(f"GRANT SELECT ON TABLE public.{object_name} TO PUBLIC")
        await _execute_admin(
            f"GRANT UPDATE (bucket) ON TABLE public.{object_name} TO {parent_role} "
            "WITH GRANT OPTION"
        )
        await _execute_admin(
            f"GRANT UPDATE (bucket) ON TABLE public.{object_name} TO {values['backup_role']} "
            "WITH GRANT OPTION"
        )
    await _execute_admin(
        f"GRANT USAGE ON SEQUENCE public.{sequence} TO {parent_role} "
        "WITH GRANT OPTION"
    )
    await _execute_admin(f"GRANT SELECT ON SEQUENCE public.{sequence} TO PUBLIC")
    await _execute_admin(f"GRANT USAGE ON SCHEMA public TO {parent_role}")
    await _execute_admin_as(
        parent_role,
        [
            f"GRANT SELECT ON TABLE public.{parent_table} TO {child_role}",
            f"GRANT SELECT ON TABLE public.{child_table} TO {child_role}",
            f"GRANT USAGE ON SEQUENCE public.{sequence} TO {child_role}",
        ],
    )
    await _execute_admin(f"REVOKE USAGE ON SCHEMA public FROM {parent_role}")

    try:
        rejected = _psql("validate_roles.sql")
        assert rejected.returncode != 0

        first_repair = _psql("bootstrap_roles.sql")
        assert first_repair.returncode == 0, first_repair.stderr
        second_repair = _psql("bootstrap_roles.sql")
        assert second_repair.returncode == 0, second_repair.stderr
        accepted = _psql("validate_roles.sql")
        assert accepted.returncode == 0, accepted.stderr

        unexpected_acls = await _fetchall_admin(
            "SELECT object.relname, COALESCE(grantee.rolname, 'PUBLIC'), "
            "acl.privilege_type, acl.is_grantable "
            "FROM pg_catalog.pg_class AS object "
            "JOIN pg_catalog.pg_namespace AS namespace "
            "ON namespace.oid = object.relnamespace "
            "CROSS JOIN LATERAL aclexplode(object.relacl) AS acl "
            "LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee "
            "WHERE namespace.nspname = 'public' "
            f"AND object.relname IN ('{parent_table}', '{child_table}', '{sequence}') "
            f"AND (acl.grantee = 0 OR grantee.rolname IN ('{parent_role}', '{child_role}'))"
        )
        assert unexpected_acls == set()
        column_acls = await _fetchall_admin(
            "SELECT object.relname, attribute.attname, "
            "COALESCE(grantee.rolname, 'PUBLIC'), acl.privilege_type, acl.is_grantable "
            "FROM pg_catalog.pg_class AS object "
            "JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace "
            "JOIN pg_catalog.pg_attribute AS attribute ON attribute.attrelid = object.oid "
            "CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl "
            "LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee "
            f"WHERE namespace.nspname = 'public' AND object.relname IN ('{parent_table}', '{child_table}')"
        )
        assert column_acls == set()
        backup_can_write_column = await _fetch_admin(
            f"SELECT has_column_privilege('{values['backup_role']}', "
            f"'public.{parent_table}', 'bucket', 'UPDATE')"
        )
        assert backup_can_write_column == (False,)
    finally:
        _psql("bootstrap_roles.sql")
        await _execute_admin_as(
            migrator,
            [
                f"DROP TABLE IF EXISTS public.{parent_table} CASCADE",
                f"DROP SEQUENCE IF EXISTS public.{sequence}",
            ],
        )
        await _execute_admin(f"DROP OWNED BY {child_role}")
        await _execute_admin(f"DROP OWNED BY {parent_role}")
        await _execute_admin(f"DROP ROLE IF EXISTS {child_role}")
        await _execute_admin(f"DROP ROLE IF EXISTS {parent_role}")


@pytest.mark.asyncio
async def test_global_and_public_default_acls_are_repaired_for_all_policy_owners():
    values = _policy_values()
    app = values["app_role"]
    migrator = values["migrator_role"]
    backup = values["backup_role"]
    unexpected = "verdaxis_unexpected_default_acl_test"
    delegated = "verdaxis_delegated_default_acl_test"
    future_table = "runtime_future_acl_test"

    await _execute_admin(f"DROP TABLE IF EXISTS public.{future_table}")
    await _execute_admin(f"DROP ROLE IF EXISTS {delegated}")
    await _execute_admin(f"DROP ROLE IF EXISTS {unexpected}")
    await _execute_admin(f"CREATE ROLE {unexpected} NOLOGIN")
    await _execute_admin(f"CREATE ROLE {delegated} NOLOGIN")

    for statement in (
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {migrator} "
        f"GRANT INSERT ON TABLES TO {unexpected}",
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {app} "
        f"GRANT EXECUTE ON FUNCTIONS TO {unexpected}",
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {backup} "
        f"GRANT USAGE ON SCHEMAS TO {unexpected}",
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {backup} "
        f"GRANT USAGE ON TYPES TO {backup} WITH GRANT OPTION",
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {app} IN SCHEMA public "
        f"GRANT USAGE ON TYPES TO {delegated}",
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {backup} IN SCHEMA public "
        f"GRANT SELECT ON SEQUENCES TO {delegated}",
    ):
        await _execute_admin(statement)

    try:
        rejected = _psql("validate_roles.sql")
        assert rejected.returncode != 0

        repaired = _psql("bootstrap_roles.sql")
        assert repaired.returncode == 0, repaired.stderr
        accepted = _psql("validate_roles.sql")
        assert accepted.returncode == 0, accepted.stderr

        await _execute_admin_as(
            migrator, [f"CREATE TABLE public.{future_table} (id bigint)"]
        )
        future_acl = await _fetchall_admin(
            "SELECT COALESCE(grantee.rolname, 'PUBLIC'), acl.privilege_type, "
            "acl.is_grantable, grantor.rolname "
            "FROM pg_catalog.pg_class AS object "
            "CROSS JOIN LATERAL aclexplode(object.relacl) AS acl "
            "LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee "
            "JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor "
            f"WHERE object.oid = 'public.{future_table}'::regclass"
        )
        owner_privileges = {
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "TRUNCATE",
            "REFERENCES",
            "TRIGGER",
            "MAINTAIN",
        }
        expected_acl = {
            (migrator, privilege, False, migrator)
            for privilege in owner_privileges
        }
        expected_acl.update(
            (app, privilege, False, migrator)
            for privilege in {"SELECT", "INSERT", "UPDATE", "DELETE"}
        )
        expected_acl.add((backup, "SELECT", False, migrator))
        assert future_acl == expected_acl
        assert _psql("validate_roles.sql").returncode == 0
    finally:
        await _execute_admin(f"DROP TABLE IF EXISTS public.{future_table}")
        _psql("bootstrap_roles.sql")
        await _execute_admin(f"DROP OWNED BY {delegated}")
        await _execute_admin(f"DROP OWNED BY {unexpected}")
        await _execute_admin(f"DROP ROLE IF EXISTS {delegated}")
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

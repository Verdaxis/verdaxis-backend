"""Disposable PostgreSQL proofs for exact runtime role authority."""

from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess
from uuid import uuid4

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


def _psql(
    script_name: str,
    *,
    url_environment: str = "POSTGRES_ADMIN_TEST_DATABASE_URL",
    single_transaction: bool = False,
) -> subprocess.CompletedProcess[str]:
    raw_url = os.environ.get(url_environment, "")
    if not raw_url:
        pytest.skip(f"{url_environment} is not configured")
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
    if single_transaction:
        command.append("--single-transaction")
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


@pytest.mark.asyncio
async def test_migrator_converges_new_policy_table_after_migration():
    values = _policy_values()
    table_name = "seed_runs"
    app_url = os.environ["DATABASE_URL"]
    migrator = values["migrator_role"]
    app = values["app_role"]
    backup = values["backup_role"]

    # Integration note: seed_runs is now a real miq-owned migration table, so
    # the probe renames it aside instead of dropping it, and restores it (and
    # the converged policy grants) afterwards. The synthetic probe table keeps
    # explicitly named constraints so the parked real table's names never
    # collide.
    parked_table = f"{table_name}_premigration_probe_parked"
    await _execute_admin_as(
        migrator,
        [
            f"DROP TABLE IF EXISTS public.{parked_table}",
            f"ALTER TABLE public.{table_name} RENAME TO {parked_table}",
            f"CREATE TABLE public.{table_name} "
            "(id bigint CONSTRAINT seed_runs_probe_pkey PRIMARY KEY, "
            "status text NOT NULL)",
            f"INSERT INTO public.{table_name} VALUES (1, 'complete')",
        ],
    )
    try:
        before_engine = create_async_engine(app_url, hide_parameters=True)
        try:
            with pytest.raises(DBAPIError):
                async with before_engine.connect() as connection:
                    await connection.execute(
                        text(f"SELECT status FROM public.{table_name}")
                    )
        finally:
            await before_engine.dispose()

        first = _psql(
            "converge_runtime_object_acls.sql",
            url_environment="MIGRATOR_DATABASE_URL",
            single_transaction=True,
        )
        assert first.returncode == 0, first.stderr
        second = _psql(
            "converge_runtime_object_acls.sql",
            url_environment="MIGRATOR_DATABASE_URL",
            single_transaction=True,
        )
        assert second.returncode == 0, second.stderr

        after_engine = create_async_engine(app_url, hide_parameters=True)
        try:
            async with after_engine.connect() as connection:
                assert (
                    await connection.execute(
                        text(f"SELECT status FROM public.{table_name}")
                    )
                ).scalar_one() == "complete"
        finally:
            await after_engine.dispose()

        for statement in (
            f"INSERT INTO public.{table_name} VALUES (2, 'forbidden')",
            f"UPDATE public.{table_name} SET status = 'forbidden'",
            f"DELETE FROM public.{table_name}",
        ):
            denied_engine = create_async_engine(app_url, hide_parameters=True)
            try:
                with pytest.raises(DBAPIError):
                    async with denied_engine.begin() as connection:
                        await connection.execute(text(statement))
            finally:
                await denied_engine.dispose()

        non_owner_acl = await _fetchall_admin(
            "SELECT COALESCE(grantee.rolname, 'PUBLIC'), acl.privilege_type, "
            "acl.is_grantable, grantor.rolname "
            "FROM pg_catalog.pg_class AS object "
            "CROSS JOIN LATERAL aclexplode(object.relacl) AS acl "
            "LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee "
            "JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor "
            f"WHERE object.oid = 'public.{table_name}'::regclass "
            "AND acl.grantee <> object.relowner"
        )
        assert non_owner_acl == {
            (app, "SELECT", False, migrator),
            (backup, "SELECT", False, migrator),
        }
    finally:
        await _execute_admin_as(
            migrator,
            [
                f"DROP TABLE IF EXISTS public.{table_name}",
                f"ALTER TABLE public.{parked_table} RENAME TO {table_name}",
            ],
        )
        _psql("bootstrap_roles.sql")


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
        expected_acl.add((backup, "SELECT", False, migrator))
        assert future_acl == expected_acl
        app_can_mutate_future = await _fetch_admin(
            f"SELECT has_table_privilege('{app}', 'public.{future_table}', "
            "'INSERT,UPDATE,DELETE')"
        )
        assert app_can_mutate_future == (False,)
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
    _policy_values()
    app_url = os.environ["DATABASE_URL"]
    backup_url = os.environ["BACKUP_DATABASE_URL"]

    backup_engine = create_async_engine(backup_url, hide_parameters=True)
    try:
        async with backup_engine.connect() as connection:
            assert (
                await connection.execute(
                    text("SELECT version_num FROM public.alembic_version")
                )
            ).scalar_one()
    finally:
        await backup_engine.dispose()

    for url, statement in (
        (app_url, "UPDATE public.alembic_version SET version_num = version_num"),
        (
            app_url,
            "UPDATE public.spatial_ref_sys SET auth_name = auth_name WHERE srid = 4326",
        ),
        (backup_url, "UPDATE public.alembic_version SET version_num = version_num"),
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
async def test_raw_app_cannot_promote_rewrite_controls_set_role_or_delegate():
    values = _policy_values()
    app = values["app_role"]
    migrator = values["migrator_role"]
    delegated = "verdaxis_raw_app_delegate_test"
    app_url = os.environ["DATABASE_URL"]
    registration_org_id = uuid4()
    registration_user_id = uuid4()

    # Integration note: organizations.provenance, seed_runs, and
    # market_row_quarantines are real migration-owned objects on the
    # linearized chain (mi/miq); the pre-integration synthetic copies are no
    # longer created or dropped here — bad grants are planted directly on the
    # real objects and repaired by bootstrap_roles.sql.
    await _execute_admin(f"DROP ROLE IF EXISTS {delegated}")
    await _execute_admin(f"CREATE ROLE {delegated} NOLOGIN")
    await _execute_admin(
        f"GRANT UPDATE (provenance) ON TABLE public.organizations TO {app} "
        "WITH GRANT OPTION"
    )
    await _execute_admin(
        f"GRANT SELECT ON TABLE public.organizations TO {app} WITH GRANT OPTION"
    )
    await _execute_admin(
        f"GRANT UPDATE (version_num) ON TABLE public.alembic_version TO {app} "
        "WITH GRANT OPTION"
    )
    await _execute_admin(
        "GRANT UPDATE (version_num) ON TABLE public.alembic_version TO PUBLIC"
    )
    await _execute_admin_as(
        app,
        [
            f"GRANT UPDATE (provenance) ON TABLE public.organizations TO {delegated}",
            f"GRANT SELECT ON TABLE public.organizations TO {delegated}",
        ],
    )

    try:
        rejected = _psql("validate_roles.sql")
        assert rejected.returncode != 0

        first_repair = _psql("bootstrap_roles.sql")
        assert first_repair.returncode == 0, first_repair.stderr
        second_repair = _psql("bootstrap_roles.sql")
        assert second_repair.returncode == 0, second_repair.stderr
        accepted = _psql("validate_roles.sql")
        assert accepted.returncode == 0, accepted.stderr

        registration_engine = create_async_engine(app_url, hide_parameters=True)
        try:
            async with registration_engine.begin() as connection:
                await connection.execute(
                    text(
                        "INSERT INTO public.organizations "
                        "(id, name, type) "
                        "VALUES (:org_id, 'Runtime ACL registration proof', "
                        "'SHIPPING_LINE')"
                    ),
                    {"org_id": registration_org_id},
                )
                await connection.execute(
                    text(
                        "INSERT INTO public.users "
                        "(id, email, password_hash, role, status, organization_id) "
                        "VALUES (:user_id, :email, 'not-a-real-hash', 'BUYER', "
                        "'PENDING', :org_id)"
                    ),
                    {
                        "user_id": registration_user_id,
                        "email": f"runtime-acl-{registration_user_id}@example.invalid",
                        "org_id": registration_org_id,
                    },
                )
                await connection.execute(
                    text(
                        "INSERT INTO public.user_status_transitions "
                        "(id, user_id, organization_id, role, from_status, "
                        "to_status, effective_at, provenance) VALUES "
                        "(gen_random_uuid(), :user_id, :org_id, 'BUYER', NULL, "
                        "'PENDING', now(), 'workflow')"
                    ),
                    {
                        "user_id": registration_user_id,
                        "org_id": registration_org_id,
                    },
                )
                await connection.execute(
                    text(
                        "INSERT INTO public.audit_logs "
                        "(id, user_id, action, resource_type, resource_id, changes) "
                        "VALUES (gen_random_uuid(), :user_id, 'USER_REGISTERED', "
                        "'user', :resource_id, '{}'::jsonb)"
                    ),
                    {
                        "user_id": registration_user_id,
                        "resource_id": str(registration_user_id),
                    },
                )
        finally:
            await registration_engine.dispose()

        admin_engine = create_async_engine(app_url, hide_parameters=True)
        try:
            async with admin_engine.begin() as connection:
                await connection.execute(
                    text(
                        "UPDATE public.users SET status = 'APPROVED' "
                        "WHERE id = :user_id"
                    ),
                    {"user_id": registration_user_id},
                )
                await connection.execute(
                    text(
                        "INSERT INTO public.user_status_transitions "
                        "(id, user_id, organization_id, role, from_status, "
                        "to_status, effective_at, provenance) VALUES "
                        "(gen_random_uuid(), :user_id, :org_id, 'BUYER', "
                        "'PENDING', 'APPROVED', now(), 'workflow')"
                    ),
                    {
                        "user_id": registration_user_id,
                        "org_id": registration_org_id,
                    },
                )
                await connection.execute(
                    text(
                        "INSERT INTO public.audit_logs "
                        "(id, user_id, action, resource_type, resource_id, changes) "
                        "VALUES (gen_random_uuid(), :user_id, 'KYC_APPROVED', "
                        "'organization', :resource_id, "
                        "'{\"verification_status\": \"APPROVED\"}'::jsonb)"
                    ),
                    {
                        "user_id": registration_user_id,
                        "resource_id": str(registration_org_id),
                    },
                )
        finally:
            await admin_engine.dispose()

        status_and_append_counts = await _fetch_admin(
            "SELECT organization.verification_status, users.status, "
            "(SELECT count(*) FROM public.audit_logs "
            f"WHERE user_id = '{registration_user_id}'), "
            "(SELECT count(*) FROM public.user_status_transitions "
            f"WHERE user_id = '{registration_user_id}') "
            "FROM public.organizations AS organization "
            "JOIN public.users AS users ON users.organization_id = organization.id "
            f"WHERE organization.id = '{registration_org_id}'"
        )
        assert status_and_append_counts == ("PENDING", "APPROVED", 2, 2)

        # organizations.verification_status is NOT rejected: the security
        # admission review endpoints (auth_simple organization approve/reject)
        # are an app-role write path, so the integrated ACL grants that single
        # column UPDATE to the app role.
        rejected_statements = (
            "UPDATE public.organizations SET provenance = 'self-promoted'",
            "INSERT INTO public.organizations "
            "(name, type, verification_status, provenance) VALUES "
            "('Unauthorized provenance', 'SHIPPING_LINE', 'PENDING', 'self')",
            "UPDATE public.alembic_version SET version_num = version_num",
            "UPDATE public.audit_logs SET action = action",
            "DELETE FROM public.audit_logs",
            "UPDATE public.user_status_transitions SET provenance = provenance",
            "DELETE FROM public.user_status_transitions",
            "UPDATE public.market_row_quarantines SET reason = 'accepted'",
            "UPDATE public.organization_market_approvals SET reason = 'accepted'",
            "DELETE FROM public.organization_market_approvals",
            "SELECT * FROM public.organization_market_approvals",
            "DELETE FROM public.seed_runs",
            f"SET ROLE {migrator}",
        )
        for statement in rejected_statements:
            engine = create_async_engine(app_url, hide_parameters=True)
            try:
                try:
                    async with engine.begin() as connection:
                        await connection.execute(text(statement))
                except DBAPIError:
                    pass
                else:
                    pytest.fail(f"raw app unexpectedly allowed: {statement}")
            finally:
                await engine.dispose()

        delegate_engine = create_async_engine(app_url, hide_parameters=True)
        try:
            async with delegate_engine.begin() as connection:
                await connection.execute(
                    text(
                        f"GRANT SELECT ON TABLE public.organizations TO {delegated}"
                    )
                )
        finally:
            await delegate_engine.dispose()
        delegated_select = await _fetch_admin(
            f"SELECT has_table_privilege('{delegated}', "
            "'public.organizations', 'SELECT')"
        )
        assert delegated_select == (False,)

        column_acls = await _fetchall_admin(
            "SELECT object.relname, attribute.attname, grantee.rolname, "
            "acl.privilege_type, acl.is_grantable "
            "FROM pg_catalog.pg_class AS object "
            "JOIN pg_catalog.pg_namespace AS namespace "
            "ON namespace.oid = object.relnamespace "
            "JOIN pg_catalog.pg_attribute AS attribute "
            "ON attribute.attrelid = object.oid "
            "CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl "
            "JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee "
            "WHERE namespace.nspname = 'public'"
        )
        insert_columns = {
            "id",
            "name",
            "domain",
            "type",
            "supplier_tier",
            "tax_id",
            "country_code",
            "created_at",
        }
        update_columns = {
            "name",
            "domain",
            "type",
            "supplier_tier",
            "tax_id",
            "country_code",
            # Integrated ACL: the security admission review endpoints
            # (auth_simple organization approve/reject) update this column
            # through the app role.
            "verification_status",
        }
        expected_column_acls = {
            ("organizations", column, app, "INSERT", False)
            for column in insert_columns
        } | {
            ("organizations", column, app, "UPDATE", False)
            for column in update_columns
        } | {
            (table, column, app, "UPDATE", False)
            for table, columns in {
                "market_support_contexts": {
                    "status",
                    "ended_at",
                    "version",
                },
                "market_support_authorizations": {
                    "status",
                    "consumed_at",
                    "revoked_at",
                    "revoked_by_actor_user_id",
                    "revocation_reason",
                },
                "staff_capability_assignments": {
                    "reason",
                    "granted_by_user_id",
                    "granted_at",
                    "expires_at",
                    "revoked_at",
                    "revoked_by_user_id",
                    "revocation_reason",
                },
            }.items()
            for column in columns
        }
        assert column_acls == expected_column_acls
    finally:
        _psql("bootstrap_roles.sql")
        await _execute_admin(
            "DELETE FROM public.audit_logs "
            f"WHERE user_id = '{registration_user_id}'"
        )
        await _execute_admin(
            "DELETE FROM public.user_status_transitions "
            f"WHERE user_id = '{registration_user_id}'"
        )
        await _execute_admin(
            f"DELETE FROM public.users WHERE id = '{registration_user_id}'"
        )
        await _execute_admin(
            f"DELETE FROM public.organizations WHERE id = '{registration_org_id}'"
        )
        await _execute_admin(f"DROP OWNED BY {delegated}")
        await _execute_admin(f"DROP ROLE IF EXISTS {delegated}")
        _psql("bootstrap_roles.sql")


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

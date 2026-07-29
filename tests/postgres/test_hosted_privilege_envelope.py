"""Prove strict Verdaxis ownership conflicts in a hosted-like PG17 envelope."""

from __future__ import annotations

import os
import re
import subprocess
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.engine import URL, make_url
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine


_IDENTIFIER = re.compile(r"^[a-z][a-z0-9_]+$")
_PASSWORD = "hosted-envelope-test-password"


def _admin_url() -> URL:
    raw_url = os.environ.get("POSTGRES_ADMIN_TEST_DATABASE_URL", "").strip()
    if not raw_url:
        pytest.skip("POSTGRES_ADMIN_TEST_DATABASE_URL is not configured")
    url = make_url(raw_url)
    if not (url.database or "").endswith("_analytics_test"):
        raise RuntimeError("refusing a non-disposable PostgreSQL database")
    return url


def _psql(statement: str, *, database: str = "postgres") -> None:
    url = _admin_url()
    result = subprocess.run(
        [
            "psql",
            "-X",
            "-h",
            url.host or "",
            "-p",
            str(url.port or 5432),
            "-U",
            url.username or "",
            "-d",
            database,
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            statement,
        ],
        env={**os.environ, "PGPASSWORD": url.password or ""},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def _sqlstate(error: DBAPIError) -> str | None:
    return getattr(error.orig, "sqlstate", None) or getattr(error.orig, "pgcode", None)


@pytest.mark.asyncio
async def test_non_owner_role_creator_cannot_reproduce_strict_runtime_ownership():
    suffix = uuid4().hex[:10]
    database = f"verdaxis_hosted_{suffix}_analytics_test"
    provisioner = f"hosted_provisioner_{suffix}"
    protected_roles = tuple(
        f"hosted_{kind}_{suffix}" for kind in ("app", "migrator", "backup")
    )
    assert all(
        _IDENTIFIER.fullmatch(value)
        for value in (database, provisioner, *protected_roles)
    )

    _psql(
        f"CREATE ROLE {provisioner} LOGIN CREATEROLE NOINHERIT "
        f"PASSWORD '{_PASSWORD}'"
    )
    _psql(f"CREATE DATABASE {database}")
    provisioner_url = _admin_url().set(
        username=provisioner,
        password=_PASSWORD,
        database=database,
    )
    engine = create_async_engine(
        provisioner_url.render_as_string(hide_password=False),
        hide_parameters=True,
    )
    try:
        async with engine.begin() as connection:
            for role in protected_roles:
                await connection.execute(
                    text(
                        f"CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB "
                        "NOCREATEROLE NOREPLICATION NOINHERIT NOBYPASSRLS"
                    )
                )

        async with engine.connect() as connection:
            memberships = {
                tuple(row)
                for row in (
                    await connection.execute(
                        text(
                            "SELECT granted.rolname, member.rolname, "
                            "membership.admin_option, membership.set_option "
                            "FROM pg_catalog.pg_auth_members AS membership "
                            "JOIN pg_catalog.pg_roles AS granted "
                            "ON granted.oid = membership.roleid "
                            "JOIN pg_catalog.pg_roles AS member "
                            "ON member.oid = membership.member "
                            f"WHERE granted.rolname IN {protected_roles!r} "
                            f"AND member.rolname = '{provisioner}'"
                        )
                    )
                ).all()
            }
        assert memberships == {
            (role, provisioner, True, False) for role in protected_roles
        }

        migrator = protected_roles[1]
        for ownership_statement in (
            f"ALTER DATABASE {database} OWNER TO {migrator}",
            f"ALTER SCHEMA public OWNER TO {migrator}",
        ):
            with pytest.raises(DBAPIError) as raised:
                async with engine.begin() as connection:
                    await connection.execute(text(ownership_statement))
            assert _sqlstate(raised.value) == "42501"

        async with engine.connect() as connection:
            database_owner, schema_owner = tuple(
                (
                    await connection.execute(
                        text(
                            "SELECT "
                            "(SELECT owner.rolname FROM pg_catalog.pg_database AS db "
                            " JOIN pg_catalog.pg_roles AS owner ON owner.oid = db.datdba "
                            f" WHERE db.datname = '{database}'), "
                            "(SELECT owner.rolname FROM pg_catalog.pg_namespace AS ns "
                            " JOIN pg_catalog.pg_roles AS owner ON owner.oid = ns.nspowner "
                            " WHERE ns.nspname = 'public')"
                        )
                    )
                ).one()
            )
        assert database_owner != migrator
        assert schema_owner != migrator
    finally:
        await engine.dispose()
        _psql(f"DROP DATABASE IF EXISTS {database} WITH (FORCE)")
        _psql(f"DROP ROLE IF EXISTS {', '.join(protected_roles)}")
        _psql(f"DROP ROLE IF EXISTS {provisioner}")

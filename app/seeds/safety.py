"""Shared fail-closed authorization and connection helpers for seed scripts."""

from __future__ import annotations

from contextlib import asynccontextmanager
from dataclasses import dataclass
from ipaddress import ip_address
import os
import re
from typing import Mapping

from sqlalchemy import text
from sqlalchemy.engine import URL, make_url
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.services.availability_windows import normalize_availability_window


_OPT_IN = "I_UNDERSTAND_SEED_MUTATIONS"
_DISPOSABLE_NAME = re.compile(r"^[a-z0-9][a-z0-9_-]*_test$")
_DENIED_DATABASES = {"verdaxis", "postgres", "template0", "template1"}
_DENIED_ROLES = {"postgres", "root"}


class SeedTargetError(RuntimeError):
    """Raised before a seeder can connect to an absent or unsafe target."""


@dataclass(frozen=True)
class SeedTarget:
    url: URL
    database_name: str
    runtime_env: str


def _is_loopback(host: str) -> bool:
    if host.lower() == "localhost":
        return True
    try:
        return ip_address(host).is_loopback
    except ValueError:
        return False


def resolve_seed_target(environ: Mapping[str, str] | None = None) -> SeedTarget:
    values = os.environ if environ is None else environ
    raw_url = values.get("SEED_DATABASE_URL", "").strip()
    if not raw_url:
        raise SeedTargetError("SEED_DATABASE_URL is required; no implicit seed target is allowed")
    if values.get("ALLOW_SEED_MUTATIONS") != _OPT_IN:
        raise SeedTargetError(f"ALLOW_SEED_MUTATIONS={_OPT_IN} is required")

    runtime_env = values.get("SEED_RUNTIME_ENV", "").strip().lower()
    if runtime_env not in {"staging", "disposable"}:
        raise SeedTargetError("SEED_RUNTIME_ENV must attest staging or disposable; production is denied")

    try:
        url = make_url(raw_url)
    except Exception as exc:
        raise SeedTargetError("SEED_DATABASE_URL must be a valid PostgreSQL URL") from exc
    if not url.drivername.startswith("postgresql"):
        raise SeedTargetError("SEED_DATABASE_URL must use PostgreSQL")

    database_name = (url.database or "").lower()
    attested_name = values.get("SEED_TARGET_DATABASE", "").strip().lower()
    if not attested_name or attested_name != database_name:
        raise SeedTargetError("SEED_TARGET_DATABASE must exactly attest the URL database")
    if database_name in _DENIED_DATABASES:
        raise SeedTargetError("production and PostgreSQL system databases are denied")
    if (url.username or "").lower() in _DENIED_ROLES:
        raise SeedTargetError("seeders must not connect with a superuser role")
    if not url.host or not _is_loopback(url.host):
        raise SeedTargetError("seeders accept only an explicitly attested loopback database target")

    if runtime_env == "staging" and database_name != "verdaxis_staging":
        raise SeedTargetError("staging seed target must be exactly verdaxis_staging")
    if runtime_env == "disposable" and not _DISPOSABLE_NAME.fullmatch(database_name):
        raise SeedTargetError("disposable seed database name must end in _test")
    return SeedTarget(url=url, database_name=database_name, runtime_env=runtime_env)


def attest_connected_seed_target(
    target: SeedTarget,
    *,
    connected_database: str,
    connected_user: str,
    connected_role_is_superuser: bool,
) -> None:
    """Fail closed if PostgreSQL connected somewhere other than the attested URL."""
    if connected_database.lower() != target.database_name:
        raise SeedTargetError("connected database does not match the attested seed target")
    if connected_user.lower() != (target.url.username or "").lower():
        raise SeedTargetError("connected current_user does not match SEED_DATABASE_URL")
    if connected_role_is_superuser:
        raise SeedTargetError("connected seed role must not be a superuser")


def seed_connection(environ: Mapping[str, str] | None = None):
    """Return a psycopg2 connection only after target authorization."""
    import psycopg2

    target = resolve_seed_target(environ)
    connection = psycopg2.connect(
        host=target.url.host,
        port=target.url.port,
        dbname=target.database_name,
        user=target.url.username,
        password=target.url.password,
    )
    try:
        with connection.cursor() as cursor:
            cursor.execute(
                "SELECT current_database(), current_user, rol.rolsuper "
                "FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user"
            )
            row = cursor.fetchone()
        attest_connected_seed_target(
            target,
            connected_database=row[0],
            connected_user=row[1],
            connected_role_is_superuser=row[2],
        )
        return connection
    except BaseException:
        connection.close()
        raise


@asynccontextmanager
async def seed_session(environ: Mapping[str, str] | None = None):
    """Yield one authorized async seed session and dispose its private engine."""
    target = resolve_seed_target(environ)
    url = target.url.set(drivername="postgresql+asyncpg")
    engine = create_async_engine(url)
    session_factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    try:
        async with session_factory() as session:
            row = (
                await session.execute(
                    text(
                        "SELECT current_database(), current_user, rol.rolsuper "
                        "FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user"
                    )
                )
            ).one()
            attest_connected_seed_target(
                target,
                connected_database=row[0],
                connected_user=row[1],
                connected_role_is_superuser=row[2],
            )
            yield session
    finally:
        await engine.dispose()


def canonical_seed_window(value: str) -> str:
    normalized = normalize_availability_window(value)
    if value != normalized:
        raise SeedTargetError(f"seed availability window must already be canonical: {value!r}")
    return value

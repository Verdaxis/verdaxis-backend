"""Real PostgreSQL upgrade/downgrade authority for fresh security bindings.

Integration note: the linearized chain ends in mi_20260720_market_integrity,
whose downgrade is unsupported by design, so the security downgrade contract
(sec_20260720_device -> sec_20260720_boundaries keeps the widened fuel width
and legacy rows intact) is exercised on a scratch database capped at the
security head instead of the session database at head.
"""

import hashlib
import os
import subprocess
import sys
from datetime import UTC, datetime
from uuid import uuid4

from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import create_async_engine

_SECURITY_HEAD = "sec_20260720_device"
_SCRATCH_DATABASE = "verdaxis_security_fresh_analytics_test"


def _admin_sql(statement: str, *, database: str = "postgres") -> None:
    url = make_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
    result = subprocess.run(
        [
            "psql", "-X",
            "-h", url.host or "",
            "-p", str(url.port or 5432),
            "-U", url.username or "",
            "-d", database,
            "-v", "ON_ERROR_STOP=1",
            "-c", statement,
        ],
        env={**os.environ, "PGPASSWORD": url.password or ""},
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


async def test_unknown_kyc_is_not_bound_and_long_fuel_survives_downgrade(analytics_pg_url):
    migrator_role = os.environ["RUNTIME_TEST_MIGRATOR_ROLE"]
    scratch_url = make_url(analytics_pg_url).set(database=_SCRATCH_DATABASE).render_as_string(
        hide_password=False
    )
    _admin_sql(f"DROP DATABASE IF EXISTS {_SCRATCH_DATABASE} WITH (FORCE)")
    _admin_sql(f"CREATE DATABASE {_SCRATCH_DATABASE} OWNER {migrator_role}")
    _admin_sql("CREATE EXTENSION IF NOT EXISTS postgis", database=_SCRATCH_DATABASE)

    environment = {
        **os.environ,
        "DATABASE_URL": scratch_url,
        "MIGRATOR_DATABASE_URL": scratch_url,
    }

    def alembic(*args: str) -> None:
        subprocess.run([sys.executable, "-m", "alembic", *args], check=True, env=environment)

    alembic("upgrade", "sec_20260720_identity")
    engine = create_async_engine(scratch_url)
    user_id = uuid4()
    organization_id = uuid4()
    inventory_id = uuid4()
    refresh_session_id = uuid4()
    refresh_family_id = uuid4()
    try:
        async with engine.begin() as connection:
            await connection.execute(
                text(
                    "INSERT INTO organizations (id, name, type, verification_status) "
                    "VALUES (:id, 'Legacy KYC Org', 'FUEL_BUYER', 'APPROVED')"
                ),
                {"id": organization_id},
            )
            await connection.execute(
                text(
                    "INSERT INTO users "
                    "(id, email, password_hash, role, status, organization_id, email_verified, "
                    "kyc_status, must_change_password, email_verification_token) VALUES "
                    "(:id, :email, 'hash', 'BUYER', 'APPROVED', :org, true, 'APPROVED', false, "
                    ":verification_token)"
                ),
                {
                    "id": user_id,
                    "email": f"legacy-{user_id}@example.test",
                    "org": organization_id,
                    "verification_token": "legacy-worker-issued-token",
                },
            )
            token_hash, expires_at = (
                await connection.execute(
                    text(
                        "SELECT email_verification_token_hash, "
                        "email_verification_token_expires_at FROM users WHERE id = :id"
                    ),
                    {"id": user_id},
                )
            ).one()
            assert token_hash == hashlib.sha256(b"legacy-worker-issued-token").hexdigest()
            assert expires_at is not None
            assert expires_at > datetime.now(UTC)

            # The boundary migration must remain blocked while the compatibility
            # link is valid, then accept the same row once its expiry has passed.
            await connection.execute(
                text(
                    "UPDATE users SET email_verification_token_expires_at = "
                    "CURRENT_TIMESTAMP - interval '1 second' WHERE id = :id"
                ),
                {"id": user_id},
            )
            await connection.execute(
                text(
                    "INSERT INTO inventory_items "
                    "(id, fuel_type, current_stock_mt, incoming_stock_mt, reserved_stock_mt) "
                    "VALUES (:id, 'Biomethane', 1, 0, 0)"
                ),
                {"id": inventory_id},
            )
            await connection.execute(
                text(
                    "INSERT INTO refresh_sessions "
                    "(id, user_id, family_id, jti_hash, expires_at) VALUES "
                    "(:id, :user_id, :family_id, :jti_hash, now() + interval '1 day')"
                ),
                {
                    "id": refresh_session_id,
                    "user_id": user_id,
                    "family_id": refresh_family_id,
                    "jti_hash": "f" * 64,
                },
            )
        alembic("upgrade", "sec_20260720_boundaries")
        alembic("upgrade", _SECURITY_HEAD)
        async with engine.connect() as connection:
            status, bound_org = (
                await connection.execute(
                    text("SELECT kyc_status, kyc_organization_id FROM users WHERE id = :id"),
                    {"id": user_id},
                )
            ).one()
            assert status == "APPROVED"
            assert bound_org is None
            device_hash, revoked = (
                await connection.execute(
                    text(
                        "SELECT device_id_hash, revoked FROM refresh_sessions WHERE id = :id"
                    ),
                    {"id": refresh_session_id},
                )
            ).one()
            assert device_hash is None
            assert revoked is True

        alembic("downgrade", "sec_20260720_boundaries")
        async with engine.connect() as connection:
            assert await connection.scalar(
                text("SELECT fuel_type FROM inventory_items WHERE id = :id"), {"id": inventory_id}
            ) == "Biomethane"
            assert await connection.scalar(
                text(
                    "SELECT character_maximum_length FROM information_schema.columns "
                    "WHERE table_name = 'inventory_items' AND column_name = 'fuel_type'"
                )
            ) == 20
    finally:
        await engine.dispose()
        _admin_sql(f"DROP DATABASE IF EXISTS {_SCRATCH_DATABASE} WITH (FORCE)")

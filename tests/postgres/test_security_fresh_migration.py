"""Real PostgreSQL upgrade/downgrade authority for fresh security bindings."""

import os
import subprocess
import sys
from uuid import uuid4

from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine


async def test_unknown_kyc_is_not_bound_and_long_fuel_survives_downgrade(analytics_pg_url):
    environment = {**os.environ, "DATABASE_URL": analytics_pg_url}

    def alembic(*args: str) -> None:
        subprocess.run([sys.executable, "-m", "alembic", *args], check=True, env=environment)

    alembic("downgrade", "sec_20260720_boundaries")
    engine = create_async_engine(analytics_pg_url)
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
                    "kyc_status, must_change_password) VALUES "
                    "(:id, :email, 'hash', 'BUYER', 'APPROVED', :org, true, 'APPROVED', false)"
                ),
                {"id": user_id, "email": f"legacy-{user_id}@example.test", "org": organization_id},
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
        alembic("upgrade", "head")
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
        alembic("upgrade", "head")

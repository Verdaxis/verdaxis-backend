"""PostgreSQL proof of the one-time KYC grace snapshot and its downgrade.

The snapshot is exercised on a scratch database capped at the grace revision,
seeded at the previous revision, so the enrollment population is exact.
"""

from __future__ import annotations

import os
import subprocess
import sys
from datetime import timedelta
from uuid import uuid4

from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import create_async_engine

_PREVIOUS_REVISION = "blng_20261014_prod"
_GRACE_REVISION = "kycg_20261014_prod"
_SCRATCH_DATABASE = "verdaxis_kyc_grace_analytics_test"


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


async def test_grace_snapshot_enrolls_only_approved_accounts_with_pending_kyc(analytics_pg_url):
    migrator_role = os.environ["RUNTIME_TEST_MIGRATOR_ROLE"]
    scratch_url = make_url(analytics_pg_url).set(database=_SCRATCH_DATABASE).render_as_string(
        hide_password=False
    )
    _admin_sql(f"DROP DATABASE IF EXISTS {_SCRATCH_DATABASE} WITH (FORCE)")
    _admin_sql(f"CREATE DATABASE {_SCRATCH_DATABASE} OWNER {migrator_role}")
    _admin_sql("CREATE EXTENSION IF NOT EXISTS postgis", database=_SCRATCH_DATABASE)
    environment = {**os.environ, "DATABASE_URL": scratch_url, "MIGRATOR_DATABASE_URL": scratch_url}

    def alembic(*args: str) -> None:
        subprocess.run([sys.executable, "-m", "alembic", *args], check=True, env=environment)

    alembic("upgrade", _PREVIOUS_REVISION)
    engine = create_async_engine(scratch_url)
    approved_org, pending_org = uuid4(), uuid4()
    # (account status, KYC status, organization) -> enrolled?
    population = {
        "pending-kyc": ("APPROVED", "PENDING", approved_org, True),
        "submitted-kyc": ("APPROVED", "SUBMITTED", approved_org, True),
        "approved-kyc": ("APPROVED", "APPROVED", approved_org, False),
        "legacy-approved": ("APPROVED", "APPROVED", approved_org, True),
        "moved-approved": ("APPROVED", "APPROVED", approved_org, False),
        "moved-pending": ("APPROVED", "PENDING", approved_org, False),
        "rejected-kyc": ("APPROVED", "REJECTED", approved_org, False),
        "legacy-kyc": ("APPROVED", "REVIEW_REQUIRED", approved_org, False),
        "pending-account": ("PENDING", "PENDING", approved_org, False),
        "pending-organization": ("APPROVED", "PENDING", pending_org, False),
        "no-organization": ("APPROVED", "PENDING", None, False),
    }
    user_ids = {label: uuid4() for label in population}
    try:
        async with engine.begin() as connection:
            for organization_id, status in ((approved_org, "APPROVED"), (pending_org, "PENDING")):
                await connection.execute(
                    text(
                        "INSERT INTO organizations (id, name, type, verification_status) "
                        "VALUES (:id, :name, 'FUEL_BUYER', :status)"
                    ),
                    {"id": organization_id, "name": f"Grace {status} {organization_id}", "status": status},
                )
            for label, (status, kyc_status, organization_id, _enrolled) in population.items():
                await connection.execute(
                    text(
                        "INSERT INTO users (id, email, password_hash, role, status, "
                        "organization_id, email_verified, kyc_status, kyc_organization_id) VALUES "
                        "(:id, :email, 'hash', 'BUYER', :status, :org, true, :kyc_status, :kyc_org)"
                    ),
                    {
                        "id": user_ids[label],
                        "email": f"{label}-{user_ids[label]}@grace.test",
                        "status": status,
                        "org": organization_id,
                        "kyc_status": kyc_status,
                        "kyc_org": pending_org if label.startswith("moved-") else approved_org if label == "approved-kyc" else None,
                    },
                )

        alembic("upgrade", _GRACE_REVISION)
        async with engine.connect() as connection:
            index_definition = await connection.scalar(text(
                "SELECT indexdef FROM pg_indexes WHERE schemaname = 'public' "
                "AND tablename = 'users' AND indexname = 'ix_users_organization_id_id'"
            ))
            assert "(organization_id, id)" in index_definition
            rows = (
                await connection.execute(
                    text(
                        "SELECT user_id, organization_id, kyc_status_at_enrollment, "
                        "enrolled_at, expires_at, created_by FROM kyc_execution_grace"
                    )
                )
            ).all()
        expected = {user_ids[label] for label, values in population.items() if values[3]}
        assert {row.user_id for row in rows} == expected
        assert {row.organization_id for row in rows} == {approved_org}
        assert {row.kyc_status_at_enrollment for row in rows} == {"PENDING", "SUBMITTED", "APPROVED"}
        # One snapshot instant and a fixed, unrenewable 14-day period.
        assert len({row.enrolled_at for row in rows}) == 1
        assert all(row.expires_at - row.enrolled_at == timedelta(days=14) for row in rows)
        assert {row.created_by for row in rows} == {_GRACE_REVISION}

        alembic("downgrade", _PREVIOUS_REVISION)
        async with engine.connect() as connection:
            assert (
                await connection.execute(text("SELECT to_regclass('public.kyc_execution_grace')"))
            ).scalar_one() is None
            assert await connection.scalar(text(
                "SELECT to_regclass('public.ix_users_organization_id_id')"
            )) is None
    finally:
        await engine.dispose()
        _admin_sql(f"DROP DATABASE IF EXISTS {_SCRATCH_DATABASE} WITH (FORCE)")

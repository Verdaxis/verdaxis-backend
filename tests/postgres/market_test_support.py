"""Superuser-only fixture setup for externally approved REAL organizations.

Production application sessions have no provenance promotion path in this
release.  PostgreSQL integration tests still need pre-approved REAL actors, so
the disposable database owner bypasses triggers for one local transaction.
The database-name guard makes this helper unusable against staging or prod.
"""

from __future__ import annotations

from collections.abc import Iterable

from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm.attributes import set_committed_value

from app.models.user import Organization, OrganizationProvenance


async def assign_fixture_real_provenance(
    session: AsyncSession,
    organizations: Iterable[Organization],
) -> None:
    rows = tuple(organizations)
    if not rows:
        return
    await session.flush()
    database_name = str(
        (await session.execute(text("SELECT current_database()"))).scalar_one()
    )
    if not database_name.endswith("_market_integrity_test"):
        raise RuntimeError(
            "fixture provenance bypass requires a disposable "
            "*_market_integrity_test database"
        )
    await session.execute(text("SET LOCAL session_replication_role = 'replica'"))
    await session.execute(
        text(
            "UPDATE organizations SET provenance = 'REAL' "
            "WHERE id = ANY(CAST(:ids AS uuid[]))"
        ),
        {"ids": [str(row.id) for row in rows]},
    )
    await session.execute(text("SET LOCAL session_replication_role = 'origin'"))
    for row in rows:
        set_committed_value(row, "provenance", OrganizationProvenance.REAL)

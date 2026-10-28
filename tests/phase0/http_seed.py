"""Synthetic HTTP fixtures; called only by a disposable runner.

The deterministic identities below are the accounts the unchanged integration
suite logs in as and mints tokens for. Two disposable runners seed them, so the
definitions live here once: the Phase 0 HTTP runner (`tests/phase0/http_runner.py`)
and the Rust mixed-contract runner (`scripts/rust_contract/seed_integration.py`).
"""
import asyncio
import os
from uuid import UUID

from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.core.security import get_password_hash
from app.models.port import Port
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from app.seeds.catalog_seed import seed_catalog
from tests.postgres.market_test_support import assign_fixture_real_provenance

# suffix, fixture name, role -- the exact rows the integration suite expects.
ITEST_IDENTITIES = (
    (11, "seller", UserRole.SUPPLIER),
    (12, "buyer", UserRole.BUYER),
    (13, "seller2", UserRole.SUPPLIER),
    (14, "buyer2", UserRole.BUYER),
    (15, "admin", UserRole.ADMIN),
)


async def add_itest_identities(
    session, password: str, *, real_provenance_on_insert: bool
) -> None:
    """Add the catalog, the Singapore port and the deterministic identities.

    `real_provenance_on_insert` selects the trusted construction route. The Rust
    mixed runner inserts REAL provenance directly, the route an administrator
    invitation takes, because the market provenance guard refuses UNKNOWN
    company writes. The Phase 0 runner instead inserts ordinary rows and then
    promotes them through `assign_fixture_real_provenance`, which keeps its
    `*_market_integrity_test` database guard.
    """
    await seed_catalog(session)
    session.add(
        Port(
            id="sg-sin",
            name="Singapore",
            country="Singapore",
            location="SRID=4326;POINT(103.8 1.3)",
            timezone="Asia/Singapore",
        )
    )
    organizations = []
    for suffix, email, role in ITEST_IDENTITIES:
        organization = Organization(
            name=f"Synthetic HTTP {email}",
            domain="example.com" if suffix == 12 else None,
            type=(
                OrgType.FUEL_SUPPLIER
                if role == UserRole.SUPPLIER
                else OrgType.FUEL_BUYER
            ),
            verification_status="APPROVED",
            provenance=(
                OrganizationProvenance.REAL
                if real_provenance_on_insert
                else OrganizationProvenance.UNKNOWN
            ),
        )
        user = User(
            id=UUID(f"9e63f7a1-0000-4000-8000-{suffix:012d}"),
            email=f"itest-{email}@example.com",
            password_hash=get_password_hash(password),
            first_name="Synthetic",
            last_name=email,
            role=role,
            status=UserStatus.APPROVED,
            email_verified=True,
            organization=organization,
            kyc_status="APPROVED",
        )
        session.add(user)
        await session.flush()
        user.kyc_organization_id = organization.id
        organizations.append(organization)
    if not real_provenance_on_insert:
        await assign_fixture_real_provenance(session, organizations)
    await session.commit()


async def main():
    engine = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            await add_itest_identities(
                session,
                os.environ["DISPOSABLE_ITEST_PASSWORD"],
                real_provenance_on_insert=False,
            )
    finally:
        await engine.dispose()


if __name__ == "__main__":
    asyncio.run(main())

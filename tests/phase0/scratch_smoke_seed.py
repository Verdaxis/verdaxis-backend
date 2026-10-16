"""Disposable fixture for the scratch business smoke: catalog, port, two actors.

The smoke adapter creates its own identities. This seed only supplies the copied
catalog, one port and the actor that the runner uses to leave a pre-existing
order for the copied-order read assertion.
"""
import asyncio
import os
from uuid import UUID, uuid4

from sqlalchemy import text
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.core.security import get_password_hash
from app.models.port import Port
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.seeds.catalog_seed import seed_catalog

RUNNER_SUPPLIER_ID = UUID("9e63f7a1-0000-4000-8000-000000000011")


async def main():
    engine = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            await seed_catalog(session)
            session.add(Port(id="sg-sin", name="Singapore", country="Singapore",
                             location="SRID=4326;POINT(103.8 1.3)", timezone="Asia/Singapore"))
            organization = Organization(
                name=f"Scratch smoke seed {uuid4().hex[:8]}", type=OrgType.FUEL_SUPPLIER,
                verification_status="APPROVED",
            )
            session.add(organization)
            await session.flush()
            password = os.environ["DISPOSABLE_ITEST_PASSWORD"]
            for suffix, email, role in ((11, "runner-supplier", UserRole.SUPPLIER),
                                        (12, "runner-buyer", UserRole.BUYER)):
                user = User(
                    id=UUID(f"9e63f7a1-0000-4000-8000-{suffix:012d}") if suffix == 11 else uuid4(),
                    email=f"{email}@example.invalid",
                    password_hash=get_password_hash(password),
                    first_name="Scratch", last_name=email, role=role,
                    status=UserStatus.APPROVED, email_verified=True,
                    organization=organization, kyc_status="APPROVED",
                )
                session.add(user)
                await session.flush()
                user.kyc_organization_id = organization.id
            await session.execute(text("SET LOCAL session_replication_role = 'replica'"))
            await session.execute(text(
                "UPDATE organizations SET provenance = 'REAL' WHERE id = :id"),
                {"id": str(organization.id)})
            await session.commit()
    finally:
        await engine.dispose()


asyncio.run(main())

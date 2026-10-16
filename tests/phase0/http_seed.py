"""Synthetic HTTP fixtures; called only by the disposable HTTP runner."""
import asyncio
import os
from uuid import UUID
from sqlalchemy.ext.asyncio import create_async_engine, async_sessionmaker
from app.models.port import Port
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.core.security import get_password_hash
from app.seeds.catalog_seed import seed_catalog
from tests.postgres.market_test_support import assign_fixture_real_provenance

async def main():
    engine = create_async_engine(os.environ['DATABASE_URL'])
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            await seed_catalog(session)
            session.add(Port(id="sg-sin", name="Singapore", country="Singapore", location="SRID=4326;POINT(103.8 1.3)", timezone="Asia/Singapore"))
            for suffix, email, role in [(11,'seller',UserRole.SUPPLIER),(12,'buyer',UserRole.BUYER),(13,'seller2',UserRole.SUPPLIER),(14,'buyer2',UserRole.BUYER),(15,'admin',UserRole.ADMIN)]:
                org = Organization(name=f'Synthetic HTTP {email}', domain='example.com' if suffix == 12 else None, type=OrgType.FUEL_SUPPLIER if role == UserRole.SUPPLIER else OrgType.FUEL_BUYER, verification_status='APPROVED')
                user = User(id=UUID(f'9e63f7a1-0000-4000-8000-{suffix:012d}'), email=f'itest-{email}@example.com', password_hash=get_password_hash(os.environ['DISPOSABLE_ITEST_PASSWORD']),first_name='Synthetic',last_name=email,role=role,status=UserStatus.APPROVED,email_verified=True,organization=org,kyc_status='APPROVED')
                session.add(user)
                await session.flush()
                user.kyc_organization_id=org.id
                await assign_fixture_real_provenance(session,[org])
            await session.commit()
    finally:
        await engine.dispose()
asyncio.run(main())

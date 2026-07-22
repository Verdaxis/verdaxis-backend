"""FOR UPDATE eligibility rechecks must observe a concurrent rejection.

Stage 6c database fix (#3): the acting user's row is already in the request
session's identity map (loaded by the auth dependency). Without
``populate_existing=True`` the locked re-SELECT returns the stale cached
attributes, so a user rejected mid-request could still pass the in-transaction
eligibility recheck and fill their own order. These tests simulate the race
with two sessions over one database: session B rejects the user after session
A has the pre-rejection row cached, then session A runs the recheck.
"""
import pytest
from types import SimpleNamespace
from uuid import uuid4

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from app.routers.trades import _revalidate_trade_parties
from app.services.market_admission import (
    MarketActorOwnership,
    lock_and_load_market_organizations,
)

REQUIRED_TABLES = ['organizations', 'users']


@pytest.fixture(scope='module')
def async_engine():
    return create_async_engine('sqlite+aiosqlite://', echo=False, future=True)


@pytest.fixture(scope='module')
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
def session_factory(async_engine, setup_tables):
    return async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )


async def _seed_approved_pair(session: AsyncSession) -> tuple[Organization, User]:
    org = Organization(
        name=f'Org {uuid4().hex[:8]}',
        type=OrgType.FUEL_SUPPLIER,
        provenance=OrganizationProvenance.REAL,
        verification_status='APPROVED',
    )
    session.add(org)
    await session.flush()
    user = User(
        email=f'{uuid4().hex}@example.com',
        password_hash='x',
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org.id,
        email_verified=True,
        kyc_status='APPROVED',
        kyc_organization_id=org.id,
    )
    session.add(user)
    await session.commit()
    return org, user


@pytest.mark.asyncio
async def test_lock_and_load_observes_concurrent_actor_rejection(session_factory):
    async with session_factory() as seed_session:
        org, user = await _seed_approved_pair(seed_session)

    async with session_factory() as request_session:
        # The auth dependency's earlier load: user enters the identity map
        # with pre-rejection attributes.
        cached = (
            await request_session.execute(select(User).where(User.id == user.id))
        ).scalar_one()
        assert cached.status == UserStatus.APPROVED

        # Admin rejects the user mid-request in another session.
        async with session_factory() as admin_session:
            target = (
                await admin_session.execute(select(User).where(User.id == user.id))
            ).scalar_one()
            target.status = UserStatus.REJECTED
            await admin_session.commit()

        with pytest.raises(HTTPException) as exc_info:
            await lock_and_load_market_organizations(
                request_session,
                [org.id],
                actor_ownerships=(MarketActorOwnership(user.id, org.id),),
            )
        assert exc_info.value.status_code == 409


@pytest.mark.asyncio
async def test_revalidate_trade_parties_observes_concurrent_rejection(session_factory):
    async with session_factory() as seed_session:
        buyer_org, buyer = await _seed_approved_pair(seed_session)
        seller_org, seller = await _seed_approved_pair(seed_session)
        buyer.role = UserRole.BUYER
        seed_session.add(buyer)
        await seed_session.commit()

    async with session_factory() as request_session:
        # Both parties cached pre-rejection. Keep strong references: the
        # identity map is weak, and the race exists precisely while the
        # request still holds the objects (as real handlers do).
        cached_parties = [
            (
                await request_session.execute(select(User).where(User.id == user_id))
            ).scalar_one()
            for user_id in (buyer.id, seller.id)
        ]
        assert all(party.kyc_status == 'APPROVED' for party in cached_parties)

        async with session_factory() as admin_session:
            target = (
                await admin_session.execute(select(User).where(User.id == seller.id))
            ).scalar_one()
            target.kyc_status = 'REJECTED'
            await admin_session.commit()

        trade = SimpleNamespace(
            buyer_user_id=buyer.id,
            seller_user_id=seller.id,
            buyer_id=buyer_org.id,
            seller_id=seller_org.id,
        )
        with pytest.raises(HTTPException) as exc_info:
            await _revalidate_trade_parties(request_session, trade)
        assert exc_info.value.status_code == 409

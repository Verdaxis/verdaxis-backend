from datetime import UTC, datetime, timedelta
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from httpx import ASGITransport, AsyncClient
from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.main import app
from app.models.orderbook import (
    Initiator,
    OrderBookOrder,
    OrderBookStatus,
    OrderCreationMethod,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.models.user import Organization, OrganizationProvenance, OrgType, User, UserRole, UserStatus
from app.routers.orderbook import _orderbook_sort_clauses
from app.routers.trades import (
    _trade_action_required_filter,
    _trade_list_filter,
    trade_summary,
)


async def _session_for(*tables):
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    async with engine.begin() as connection:
        await connection.run_sync(lambda sync: Base.metadata.create_all(sync, tables=list(tables)))
    return engine, async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)


def _order(*, price: str, remaining: str, created_at: datetime) -> OrderBookOrder:
    return OrderBookOrder(
        id=uuid4(),
        organization_id=uuid4(),
        side=OrderSide.BID,
        product_id=uuid4(),
        delivery_point_id=uuid4(),
        quantity_mt=remaining,
        remaining_quantity_mt=remaining,
        price_per_mt_usd=price,
        availability_window="SPOT",
        status=OrderBookStatus.OPEN,
        provenance=OrganizationProvenance.REAL,
        created_at=created_at,
    )


def _eligible_user(user_id, organization_id, role):
    return User(
        id=user_id,
        email=f"{user_id}@example.invalid",
        password_hash="hash",
        organization_id=organization_id,
        role=role,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
        kyc_status="PENDING",
    )


def _approved_org(organization_id):
    return Organization(
        id=organization_id,
        name=str(organization_id),
        type=OrgType.FUEL_BUYER,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )


@pytest.mark.asyncio
async def test_orderbook_sort_is_global_before_pagination_and_uses_remaining_quantity():
    engine, session_factory = await _session_for(OrderBookOrder.__table__)
    try:
        created = datetime(2026, 9, 7, tzinfo=UTC)
        orders = [
            _order(price="300", remaining="10", created_at=created + timedelta(minutes=1)),
            _order(price="100", remaining="20", created_at=created + timedelta(minutes=2)),
            _order(price="200", remaining="90", created_at=created + timedelta(minutes=3)),
            _order(price="100", remaining="30", created_at=created + timedelta(minutes=4)),
            _order(price="250", remaining="40", created_at=created + timedelta(minutes=5)),
        ]
        async with session_factory() as session:
            session.add_all(orders)
            await session.commit()
            page_one = (
                await session.execute(
                    select(OrderBookOrder.__table__)
                    .order_by(*_orderbook_sort_clauses("price_asc"))
                    .offset(0)
                    .limit(2)
                )
            ).all()
            page_two = (
                await session.execute(
                    select(OrderBookOrder.__table__)
                    .order_by(*_orderbook_sort_clauses("price_asc"))
                    .offset(2)
                    .limit(2)
                )
            ).all()
            ordered = page_one + page_two
            assert [order.price_per_mt_usd for order in ordered] == [
                Decimal("100"),
                Decimal("100"),
                Decimal("200"),
                Decimal("250"),
            ]

            quantity_order = (
                await session.execute(
                    select(OrderBookOrder.__table__).order_by(*_orderbook_sort_clauses("quantity_desc"))
                )
            ).first()
            assert quantity_order.remaining_quantity_mt == 90
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_invalid_orderbook_sort_is_rejected_before_database_access():
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get("/api/orderbook/bids", params={"sort_by": "oldest"})
    assert response.status_code == 422


@pytest.mark.asyncio
async def test_trade_summary_and_action_filter_are_tenant_scoped(monkeypatch):
    engine, session_factory = await _session_for(
        Trade.__table__,
        OrderBookOrder.__table__,
        Organization.__table__,
        User.__table__,
    )
    try:
        buyer_org, seller_org, foreign_org = uuid4(), uuid4(), uuid4()
        buyer_user, seller_user = uuid4(), uuid4()
        buyer_db_user = _eligible_user(buyer_user, buyer_org, UserRole.BUYER)
        seller_db_user = _eligible_user(seller_user, seller_org, UserRole.SUPPLIER)
        current_user = SimpleNamespace(
            id=buyer_user,
            organization_id=buyer_org,
            role=UserRole.BUYER,
            status=UserStatus.APPROVED,
            email_verified=True,
            must_change_password=False,
            kyc_status="PENDING",
            kyc_organization_id=None,
        )

        def trade(*, buyer_id, seller_id, initiator_org_id, initiated_by, status, buyer_user_id=buyer_user):
            return Trade(
                id=uuid4(),
                buyer_id=buyer_id,
                seller_id=seller_id,
                buyer_user_id=buyer_user_id,
                seller_user_id=seller_user,
                initiator_org_id=initiator_org_id,
                initiated_by=initiated_by,
                status=status,
                buyer_provenance=OrganizationProvenance.REAL,
                seller_provenance=OrganizationProvenance.REAL,
                quantity_mt=100,
                price_per_mt_usd=500,
            )

        rows = [
            trade(
                buyer_id=buyer_org,
                seller_id=seller_org,
                initiator_org_id=buyer_org,
                initiated_by=Initiator.BUYER,
                status=TradeStatus.PENDING_CONFIRMATION,
            ),
            trade(
                buyer_id=buyer_org,
                seller_id=seller_org,
                initiator_org_id=seller_org,
                initiated_by=Initiator.SELLER,
                status=TradeStatus.PENDING_CONFIRMATION,
            ),
            trade(
                buyer_id=buyer_org,
                seller_id=seller_org,
                initiator_org_id=buyer_org,
                initiated_by=Initiator.BUYER,
                status=TradeStatus.CONFIRMED,
            ),
            trade(
                buyer_id=foreign_org,
                seller_id=foreign_org,
                initiator_org_id=seller_org,
                initiated_by=Initiator.SELLER,
                status=TradeStatus.PENDING_CONFIRMATION,
                buyer_user_id=uuid4(),
            ),
            trade(
                buyer_id=buyer_org,
                seller_id=seller_org,
                initiator_org_id=seller_org,
                initiated_by=Initiator.SELLER,
                status=TradeStatus.DELIVERED,
            ),
        ]
        rows.extend(
            trade(
                buyer_id=buyer_org,
                seller_id=seller_org,
                initiator_org_id=buyer_org,
                initiated_by=Initiator.BUYER,
                status=TradeStatus.CONFIRMED,
            )
            for _ in range(21)
        )
        async with session_factory() as session:
            session.add_all([
                _approved_org(buyer_org),
                _approved_org(seller_org),
                buyer_db_user,
                seller_db_user,
                *rows,
            ])
            await session.commit()

            async def resolve_self_service_party(request, db, user):
                return _self_service_party()

            monkeypatch.setattr(
                "app.routers.trades.resolve_request_party",
                resolve_self_service_party,
            )
            assert all(
                row.buyer_id != buyer_org or row.seller_id != buyer_org
                for row in rows[3:]
            )
            assert (
                await session.execute(
                    select(Trade.id).where(
                        or_(Trade.buyer_id == buyer_org, Trade.seller_id == buyer_org)
                    )
                )
            ).scalars().all() == [
                row.id
                for row in rows
                if row.buyer_id == buyer_org or row.seller_id == buyer_org
            ]
            summary = await trade_summary(SimpleNamespace(headers={}), session, current_user)
            assert summary.model_dump() == {
                "total_count": 25,
                "action_required_count": 1,
                "awaiting_counterparty_count": 1,
                "confirmed_count": 23,
            }

            visible_ids = (
                await session.execute(
                    select(Trade.id).where(
                        or_(Trade.buyer_id == buyer_org, Trade.seller_id == buyer_org),
                        _trade_action_required_filter(current_user, buyer_org),
                    )
                )
            ).scalars().all()
            assert visible_ids == [rows[1].id]

            active_ids = (
                await session.execute(
                    select(Trade.id).where(
                        _trade_list_filter(
                            current_user,
                            buyer_org,
                            action_required=False,
                            status_group="active",
                        )
                    )
                )
            ).scalars().all()
            assert set(active_ids) == {
                row.id
                for row in rows
                if row.buyer_id == buyer_org or row.seller_id == buyer_org
                if row.status == TradeStatus.PENDING_CONFIRMATION
            }

            completed_ids = (
                await session.execute(
                    select(Trade.id).where(
                        _trade_list_filter(
                            current_user,
                            buyer_org,
                            action_required=False,
                            status_group="completed",
                        )
                    )
                )
            ).scalars().all()
            assert set(completed_ids) == {
                row.id
                for row in rows
                if row.buyer_id == buyer_org or row.seller_id == buyer_org
                if row.status in {
                    TradeStatus.CONFIRMED,
                    TradeStatus.DELIVERED,
                    TradeStatus.PAID,
                }
            }
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_assisted_action_filter_requires_side_role_and_admitted_owner():
    engine, session_factory = await _session_for(
        Trade.__table__,
        OrderBookOrder.__table__,
        Organization.__table__,
        User.__table__,
    )
    try:
        buyer_org, seller_org = uuid4(), uuid4()
        buyer_user_id, seller_user_id = uuid4(), uuid4()
        buyer_support_admin_id, seller_support_admin_id = uuid4(), uuid4()
        bid_id, ask_id = uuid4(), uuid4()
        buyer_user = _eligible_user(buyer_user_id, buyer_org, UserRole.BUYER)
        seller_user = _eligible_user(seller_user_id, seller_org, UserRole.SUPPLIER)
        buyer_support_admin = _eligible_user(
            buyer_support_admin_id, buyer_org, UserRole.ADMIN
        )
        seller_support_admin = _eligible_user(
            seller_support_admin_id, seller_org, UserRole.ADMIN
        )
        bid = _order(price="500", remaining="100", created_at=datetime.now(UTC))
        bid.id = bid_id
        bid.organization_id = buyer_org
        bid.owner_user_id = buyer_support_admin_id
        bid.created_by_actor_user_id = buyer_support_admin_id
        bid.creation_method = OrderCreationMethod.MARKET_SUPPORT
        ask = _order(price="500", remaining="100", created_at=datetime.now(UTC))
        ask.id = ask_id
        ask.organization_id = seller_org
        ask.side = OrderSide.ASK
        ask.owner_user_id = seller_support_admin_id
        ask.created_by_actor_user_id = seller_support_admin_id
        ask.creation_method = OrderCreationMethod.MARKET_SUPPORT
        seller_action = Trade(
            id=uuid4(),
            ask_order_id=ask_id,
            buyer_id=buyer_org,
            seller_id=seller_org,
            buyer_user_id=buyer_user_id,
            seller_user_id=seller_support_admin_id,
            initiator_org_id=buyer_org,
            initiated_by=Initiator.BUYER,
            status=TradeStatus.PENDING_CONFIRMATION,
            buyer_provenance=OrganizationProvenance.REAL,
            seller_provenance=OrganizationProvenance.REAL,
            quantity_mt=100,
            price_per_mt_usd=500,
        )
        buyer_action = Trade(
            id=uuid4(),
            bid_order_id=bid_id,
            buyer_id=buyer_org,
            seller_id=seller_org,
            buyer_user_id=buyer_support_admin_id,
            seller_user_id=seller_user_id,
            initiator_org_id=seller_org,
            initiated_by=Initiator.SELLER,
            status=TradeStatus.PENDING_CONFIRMATION,
            buyer_provenance=OrganizationProvenance.REAL,
            seller_provenance=OrganizationProvenance.REAL,
            quantity_mt=100,
            price_per_mt_usd=500,
        )
        malformed_seller_action = Trade(
            id=uuid4(),
            ask_order_id=ask_id,
            buyer_id=buyer_org,
            seller_id=seller_org,
            buyer_user_id=buyer_user_id,
            seller_user_id=seller_user_id,
            initiator_org_id=buyer_org,
            initiated_by=Initiator.BUYER,
            status=TradeStatus.PENDING_CONFIRMATION,
            buyer_provenance=OrganizationProvenance.REAL,
            seller_provenance=OrganizationProvenance.REAL,
            quantity_mt=100,
            price_per_mt_usd=500,
        )
        malformed_buyer_action = Trade(
            id=uuid4(),
            bid_order_id=bid_id,
            buyer_id=buyer_org,
            seller_id=seller_org,
            buyer_user_id=buyer_user_id,
            seller_user_id=seller_user_id,
            initiator_org_id=seller_org,
            initiated_by=Initiator.SELLER,
            status=TradeStatus.PENDING_CONFIRMATION,
            buyer_provenance=OrganizationProvenance.REAL,
            seller_provenance=OrganizationProvenance.REAL,
            quantity_mt=100,
            price_per_mt_usd=500,
        )
        async with session_factory() as session:
            session.add_all([
                _approved_org(buyer_org),
                _approved_org(seller_org),
                buyer_user,
                seller_user,
                buyer_support_admin,
                seller_support_admin,
                bid,
                ask,
                seller_action,
                buyer_action,
                malformed_seller_action,
                malformed_buyer_action,
            ])
            await session.commit()

            supplier_view = SimpleNamespace(
                id=seller_user_id,
                organization_id=seller_org,
                role=UserRole.SUPPLIER,
                status=UserStatus.APPROVED,
                email_verified=True,
                must_change_password=False,
                kyc_status="PENDING",
                kyc_organization_id=None,
            )
            buyer_view = SimpleNamespace(
                id=buyer_user_id,
                organization_id=buyer_org,
                role=UserRole.BUYER,
                status=UserStatus.APPROVED,
                email_verified=True,
                must_change_password=False,
                kyc_status="PENDING",
                kyc_organization_id=None,
            )
            wrong_role_view = SimpleNamespace(**{**supplier_view.__dict__, "role": UserRole.BUYER})
            wrong_org_view = SimpleNamespace(
                **{**supplier_view.__dict__, "organization_id": buyer_org}
            )

            async def matching_ids(view, org_id):
                return (
                    await session.execute(
                        select(Trade.id).where(_trade_action_required_filter(view, org_id))
                    )
                ).scalars().all()

            assert await matching_ids(supplier_view, seller_org) == [seller_action.id]
            assert await matching_ids(buyer_view, buyer_org) == [buyer_action.id]
            assert await matching_ids(wrong_role_view, seller_org) == []
            assert await matching_ids(wrong_org_view, seller_org) == []

            seller_support_admin.status = UserStatus.REJECTED
            await session.commit()
            assert await matching_ids(supplier_view, seller_org) == []
    finally:
        await engine.dispose()


def _self_service_party():
    return SimpleNamespace(effective_organization=None)

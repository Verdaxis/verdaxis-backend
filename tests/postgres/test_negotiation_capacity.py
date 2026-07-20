"""PostgreSQL authority for bounded, atomic negotiation capacity consumption."""

import asyncio
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.models.catalog import Product
from app.models.negotiation import Negotiation, NegotiationStatus
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.routers import negotiations


async def _user(session, org, role, label):
    user = User(
        id=uuid4(),
        email=f"{label}-{uuid4()}@example.test",
        password_hash="hash",
        role=role,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=org.id,
    )
    session.add(user)
    await session.flush()
    return user


async def test_concurrent_acceptance_cannot_overfill_one_canonical_order(pg_session):
    engine, session = pg_session
    seller_org = Organization(id=uuid4(), name="Capacity Seller", type=OrgType.FUEL_SUPPLIER)
    buyer_orgs = [
        Organization(id=uuid4(), name=f"Capacity Buyer {index}", type=OrgType.FUEL_BUYER)
        for index in range(2)
    ]
    product = Product(
        id=uuid4(),
        name=f"Capacity Product {uuid4()}",
        fuel_type="Methanol",
        fuel_grade="Bio",
    )
    session.add_all([seller_org, *buyer_orgs, product])
    await session.flush()
    seller = await _user(session, seller_org, UserRole.SUPPLIER, "seller")
    buyers = [
        await _user(session, org, UserRole.BUYER, f"buyer-{index}")
        for index, org in enumerate(buyer_orgs)
    ]
    ask = OrderBookOrder(
        organization_id=seller_org.id,
        owner_user_id=seller.id,
        side=OrderSide.ASK,
        product_id=product.id,
        quantity_mt=Decimal("10"),
        remaining_quantity_mt=Decimal("10"),
        price_per_mt_usd=Decimal("100"),
        certification_declared=True,
        certification_scheme="ISCC",
        status=OrderBookStatus.OPEN,
    )
    session.add(ask)
    await session.flush()
    ask_id = ask.id
    negotiation_rows = []
    for buyer, buyer_org in zip(buyers, buyer_orgs, strict=True):
        row = Negotiation(
            ask_order_id=ask.id,
            initiator_org_id=buyer_org.id,
            counterparty_org_id=seller_org.id,
            initiator_user_id=buyer.id,
            counterparty_user_id=seller.id,
            initiator_side="BUYER",
            product_id=product.id,
            quantity_mt=Decimal("7"),
            current_price=Decimal("100"),
            status=NegotiationStatus.OPEN,
            last_actor_org_id=buyer_org.id,
            expires_at=datetime.now(UTC) + timedelta(hours=1),
        )
        session.add(row)
        negotiation_rows.append(row)
    await session.commit()

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    first_consumed = asyncio.Event()
    release_first = asyncio.Event()

    async def consume(index: int) -> str:
        async with factory() as worker:
            neg = await worker.get(Negotiation, negotiation_rows[index].id)
            try:
                await negotiations._consume_negotiation_capacity(worker, neg)
                if index == 0:
                    first_consumed.set()
                    await release_first.wait()
                await worker.commit()
                return "consumed"
            except HTTPException as exc:
                await worker.rollback()
                return exc.detail["code"]

    first = asyncio.create_task(consume(0))
    await asyncio.wait_for(first_consumed.wait(), timeout=2)
    second = asyncio.create_task(consume(1))
    second_result = await asyncio.wait_for(second, timeout=2)
    release_first.set()
    first_result = await asyncio.wait_for(first, timeout=2)

    assert {first_result, second_result} == {"consumed", "NEGOTIATION_CAPACITY_BUSY"}
    session.expire_all()
    remaining, order_status = (
        await session.execute(
            select(OrderBookOrder.remaining_quantity_mt, OrderBookOrder.status).where(
                OrderBookOrder.id == ask_id
            )
        )
    ).one()
    assert remaining == Decimal("3.00")
    assert order_status == OrderBookStatus.PARTIALLY_FILLED

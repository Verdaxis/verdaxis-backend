"""PostgreSQL race authority for lifecycle-correct security invalidation."""

from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import select
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.models.audit import AuditLog
from app.models.catalog import Product
from app.models.marketplace import FuelType, InventoryItem
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.services.execution_invalidation import invalidate_execution_state


async def test_invalidation_race_rolls_back_then_conserves_unfilled_reservation(pg_session):
    engine, session = pg_session
    org = Organization(id=uuid4(), name=f"Inventory Org {uuid4()}", type=OrgType.FUEL_SUPPLIER)
    product = Product(id=uuid4(), name=f"Inventory Product {uuid4()}", fuel_type="Methanol", fuel_grade="Bio")
    session.add_all([org, product])
    await session.flush()
    user = User(
        id=uuid4(), email=f"inventory-{uuid4()}@example.test", password_hash="hash",
        role=UserRole.SUPPLIER, status=UserStatus.APPROVED, email_verified=True,
        organization_id=org.id,
    )
    inventory = InventoryItem(
        id=uuid4(), supplier_id=org.id, fuel_type=FuelType.Methanol,
        current_stock_mt=Decimal("10"), reserved_stock_mt=Decimal("10"),
        price_per_mt_usd=Decimal("100"),
    )
    session.add_all([user, inventory])
    await session.flush()
    order = OrderBookOrder(
        organization_id=org.id, owner_user_id=user.id, inventory_item_id=inventory.id,
        side=OrderSide.ASK, product_id=product.id, quantity_mt=Decimal("10"),
        remaining_quantity_mt=Decimal("4"), price_per_mt_usd=Decimal("100"),
        certification_declared=True, certification_scheme="ISCC",
        status=OrderBookStatus.PARTIALLY_FILLED,
    )
    session.add(order)
    await session.commit()
    user_id = user.id
    order_id = order.id
    inventory_id = inventory.id

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as competing:
        locked = await competing.scalar(
            select(OrderBookOrder).where(OrderBookOrder.id == order_id).with_for_update()
        )
        locked.remaining_quantity_mt = Decimal("3")
        with pytest.raises(DBAPIError):
            await invalidate_execution_state(session, user_ids=[user_id], actor_user_id=user_id)
        await session.rollback()
        await competing.commit()

    result = await invalidate_execution_state(session, user_ids=[user_id], actor_user_id=user_id)
    assert result["orders_cancelled"] == 1
    assert await session.scalar(
        select(AuditLog).where(AuditLog.resource_id == str(order_id), AuditLog.action == "order.cancelled")
    ) is not None
    await session.commit()

    session.expire_all()
    assert await session.scalar(
        select(OrderBookOrder.status).where(OrderBookOrder.id == order_id)
    ) == OrderBookStatus.CANCELLED
    assert await session.scalar(
        select(InventoryItem.reserved_stock_mt).where(InventoryItem.id == inventory_id)
    ) == Decimal("7.00")

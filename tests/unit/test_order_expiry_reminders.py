"""Exercise expiry boundaries, owner privacy, and repeated scans on a disposable DB."""

from datetime import UTC, datetime, timedelta
from uuid import uuid4

import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.database import Base
from app.models.notification import Notification
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserStatus,
)
from app.services.order_expiry_reminders import remind_expiring_orders


@pytest.mark.asyncio
async def test_reminders_cover_due_real_orders_once_and_preserve_expiry():
    engine = create_async_engine("sqlite+aiosqlite://")
    async with engine.begin() as connection:
        await connection.run_sync(
            Base.metadata.create_all,
            tables=[
                Organization.__table__,
                User.__table__,
                OrderBookOrder.__table__,
                Notification.__table__,
            ],
        )
    now = datetime(2026, 9, 11, 12, tzinfo=UTC)
    async with async_sessionmaker(engine, expire_on_commit=False)() as db:
        org = Organization(
            id=uuid4(),
            name="Real test company",
            type=OrgType.FUEL_BUYER,
            provenance=OrganizationProvenance.REAL,
        )
        owner = User(
            id=uuid4(),
            email="owner@example.test",
            password_hash="unused",
            organization_id=org.id,
            status=UserStatus.APPROVED,
        )
        colleague = User(
            id=uuid4(),
            email="colleague@example.test",
            password_hash="unused",
            organization_id=org.id,
            status=UserStatus.APPROVED,
        )
        db.add_all([org, owner, colleague])
        await db.flush()
        orders = []
        for hours, status, provenance in [
            (48, OrderBookStatus.OPEN, OrganizationProvenance.REAL),
            (1, OrderBookStatus.PARTIALLY_FILLED, OrganizationProvenance.REAL),
            (49, OrderBookStatus.OPEN, OrganizationProvenance.REAL),
            (0, OrderBookStatus.OPEN, OrganizationProvenance.REAL),
            (1, OrderBookStatus.CANCELLED, OrganizationProvenance.REAL),
            (1, OrderBookStatus.FILLED, OrganizationProvenance.REAL),
            (1, OrderBookStatus.OPEN, OrganizationProvenance.DEMO),
        ]:
            order = OrderBookOrder(
                id=uuid4(),
                organization_id=org.id,
                owner_user_id=owner.id,
                side=OrderSide.BID,
                product_id=uuid4(),
                quantity_mt=100,
                remaining_quantity_mt=50,
                price_per_mt_usd=500,
                availability_window="SPOT",
                status=status,
                provenance=provenance,
                expires_at=now + timedelta(hours=hours),
            )
            db.add(order)
            orders.append(order)
        await db.commit()
        assert await remind_expiring_orders(db, now=now, batch_size=1) == 1
        await db.commit()
        assert await remind_expiring_orders(db, now=now, batch_size=1) == 1
        await db.commit()
        assert await remind_expiring_orders(db, now=now) == 0
        notifications = (await db.scalars(select(Notification))).all()
        assert len(notifications) == 2
        assert {notice.recipient_id for notice in notifications} == {owner.id}
        assert {notice.data["order_id"] for notice in notifications} == {
            str(order.id) for order in orders[:2]
        }
        assert orders[0].expires_at == now + timedelta(hours=48)
        for notice in notifications:
            notice.is_read = True
        await db.commit()
        assert await remind_expiring_orders(db, now=now) == 0
        # A changed expiry is a new reminder occasion; regular scans do not change it.
        orders[0].expires_at = now + timedelta(hours=47)
        await db.commit()
        assert await remind_expiring_orders(db, now=now) == 1
        await db.commit()
    await engine.dispose()

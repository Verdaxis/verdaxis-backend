"""Exercise expiry boundaries, owner privacy, and repeated scans on a disposable DB."""

from datetime import UTC, datetime, timedelta
from uuid import uuid4
from unittest.mock import AsyncMock

import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.database import Base
from app.models.notification import Notification
from app.models.user_preference import UserPreference
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserStatus,
)
from app.services.order_expiry_reminders import (
    remind_expiring_orders,
    deliver_expiry_reminder_emails,
)
from app.routers.notifications import (
    NotificationResponse,
    get_notifications,
    get_unread_count,
)


@pytest.mark.asyncio
async def test_reminders_cover_due_real_orders_once_and_preserve_expiry(monkeypatch):
    send_email = AsyncMock(return_value=False)
    monkeypatch.setattr(
        "app.services.order_expiry_reminders.send_order_expiry_email", send_email
    )
    engine = create_async_engine("sqlite+aiosqlite://")
    async with engine.begin() as connection:
        await connection.run_sync(
            Base.metadata.create_all,
            tables=[
                Organization.__table__,
                User.__table__,
                OrderBookOrder.__table__,
                Notification.__table__,
                UserPreference.__table__,
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
            email_verified=True,
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
        assert await deliver_expiry_reminder_emails(db, now=now) == 0
        await db.commit()
        # Two valid emails retry; the old-expiry email is discarded before sending.
        assert send_email.await_count == 2
        frozen_calls = list(send_email.call_args_list)
        assert await deliver_expiry_reminder_emails(db, now=now) == 0
        assert send_email.await_count == 2
        send_email.return_value = True
        assert (
            await deliver_expiry_reminder_emails(db, now=now + timedelta(minutes=15))
            == 2
        )
        await db.commit()
        assert send_email.call_args_list[2:] == frozen_calls
        assert (
            await deliver_expiry_reminder_emails(db, now=now + timedelta(minutes=30))
            == 0
        )
        assert send_email.await_count == 4
        # A later due order is not emailed to a stale address after an account update.
        assert await remind_expiring_orders(db, now=now + timedelta(hours=1)) == 1
        await db.commit()
        owner.email = "changed@example.test"
        await db.commit()
        assert (
            await deliver_expiry_reminder_emails(db, now=now + timedelta(hours=1)) == 0
        )
        await db.commit()
        assert send_email.await_count == 4
        notice = (await db.scalars(select(Notification))).first()
        public = NotificationResponse.model_validate(notice).model_dump()
        assert public["data"]["event"] == "order_expiring"
        assert not any(key.startswith("_email_") for key in public["data"])

        terminal = (await db.scalars(select(Notification))).all()
        assert all("_email_payload" not in item.data for item in terminal)
        visible_count = len(
            await get_notifications(skip=0, limit=100, current_user=owner, db=db)
        )
        unread_count = (await get_unread_count(current_user=owner, db=db))["count"]
        preferences = UserPreference(
            user_id=owner.id,
            namespace="notifications",
            updated_at=now,
            value={"email_trade_updates": False, "inapp_trade_updates": False},
        )
        db.add(preferences)
        orders[0].expires_at = now + timedelta(hours=46)
        await db.commit()
        assert await remind_expiring_orders(db, now=now) == 1
        await db.commit()
        assert await deliver_expiry_reminder_emails(db, now=now) == 0
        assert send_email.await_count == 4
        assert (
            len(await get_notifications(skip=0, limit=100, current_user=owner, db=db))
            == visible_count
        )
        assert (await get_unread_count(current_user=owner, db=db))[
            "count"
        ] == unread_count
        # Email-only reminders remain out of the bell and unread count.
        preferences.value = {"email_trade_updates": True, "inapp_trade_updates": False}
        orders[0].expires_at = now + timedelta(hours=45)
        await db.commit()
        assert await remind_expiring_orders(db, now=now) == 1
        await db.commit()
        # Recheck an opt-out applied after the message was queued.
        preferences.value = {"email_trade_updates": False, "inapp_trade_updates": False}
        await db.commit()
        assert await deliver_expiry_reminder_emails(db, now=now) == 0
        assert send_email.await_count == 4
        assert (
            len(await get_notifications(skip=0, limit=100, current_user=owner, db=db))
            == visible_count
        )
    await engine.dispose()

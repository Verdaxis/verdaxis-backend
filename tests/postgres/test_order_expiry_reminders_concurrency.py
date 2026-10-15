"""PostgreSQL concurrency proofs for order-expiry reminder dedup and leases.

SQLite cannot exercise the production-only ``pg_try_advisory_xact_lock`` scan
guard or the ``FOR UPDATE SKIP LOCKED`` delivery claim, so the sequential unit
tests would pass even if either protection were removed. These tests use two
independent sessions against the disposable PostgreSQL 17 database to prove:

* two concurrent scans still write one reminder per order, recipient and
  expiry;
* two concurrent claimers send one email for one pending row, with the same
  frozen payload and notification-ID provider key;
* an active retry lease is respected, an expired lease is reclaimable, and
  copied rows keep the inbox and unread-count privacy contract.
"""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime, timedelta
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.notification import Notification, NotificationType
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserStatus,
)
from app.routers.notifications import get_notifications, get_unread_count
from app.services.order_expiry_reminders import (
    deliver_expiry_reminder_emails,
    remind_expiring_orders,
)

NOW = datetime(2026, 10, 16, 12, 0, 0, tzinfo=UTC)
EXPIRY = NOW + timedelta(hours=24)
REMINDER_EVENT = "order_expiring"


def _factory(engine) -> async_sessionmaker:
    return async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)


async def _seed(session, *, tenant_count: int, owner_only: bool):
    """Create one REAL organization with approved, verified users and one order.

    With ``owner_only`` the order has a single recipient; otherwise it is
    unowned, so every approved organization member is a recipient.
    """
    org = Organization(
        id=uuid4(),
        name=f"Synthetic reminder company {uuid4().hex[:8]}",
        type=OrgType.FUEL_BUYER,
        verification_status="APPROVED",
        # INSERT of REAL is permitted by the provenance trigger; only an
        # UPDATE is guarded, so no session_replication_role bypass is needed.
        provenance=OrganizationProvenance.REAL,
    )
    product_spec = PRODUCTS_BY_CODE["BIO_METHANOL"]
    point_spec = DELIVERY_POINTS_BY_NAME["Singapore"]
    product = Product(
        id=product_spec.id,
        name=product_spec.name,
        fuel_type=product_spec.fuel_type,
        fuel_grade=product_spec.fuel_grade,
        unit=product_spec.unit,
        min_lot_size=product_spec.min_lot_size,
        is_active=True,
    )
    point = DeliveryPoint(
        id=point_spec.id,
        name=point_spec.name,
        region=point_spec.region,
        timezone=point_spec.timezone,
        is_active=True,
    )
    users = [
        User(
            id=uuid4(),
            email=f"reminder-{uuid4().hex[:8]}@example.test",
            email_verified=True,
            password_hash="unused",
            organization_id=org.id,
            status=UserStatus.APPROVED,
        )
        for _ in range(tenant_count)
    ]
    order = OrderBookOrder(
        id=uuid4(),
        organization_id=org.id,
        owner_user_id=users[0].id if owner_only else None,
        side=OrderSide.BID,
        product_id=product.id,
        delivery_point_id=point.id,
        quantity_mt=100,
        remaining_quantity_mt=100,
        price_per_mt_usd=500,
        availability_window="SPOT",
        status=OrderBookStatus.OPEN,
        provenance=OrganizationProvenance.REAL,
        expires_at=EXPIRY,
    )
    session.add_all([product, point, org, *users])
    # The owner FK is not relationship-mapped, so flush users before the order.
    await session.flush()
    session.add(order)
    await session.commit()
    return org, users, order


def _frozen_payload(user: User) -> dict:
    return {
        "to": [user.email],
        "subject": f"Order {uuid4()} expires soon",
        "html": "<p>Synthetic reminder</p>",
    }


def _copied_row(user: User, order: OrderBookOrder, *, payload: dict, retry_at: float, deadline: float) -> Notification:
    return Notification(
        id=uuid4(),
        recipient_id=user.id,
        type=NotificationType.ORDER_UPDATE,
        title="Copied reminder",
        message="Synthetic",
        data={
            "event": REMINDER_EVENT,
            "order_id": str(order.id),
            "order_key": order.id.hex,
            "expiry_epoch": EXPIRY.timestamp(),
            "expires_at": EXPIRY.isoformat(),
            "_email_only": True,
            "_email_status": "pending",
            "_email_payload": payload,
            "_email_retry_at": retry_at,
            "_email_deadline": deadline,
        },
        created_at=NOW,
    )


@pytest.mark.asyncio
async def test_two_concurrent_scans_write_one_reminder_per_recipient(pg_session, monkeypatch):
    engine, seed = pg_session
    org, users, order = await _seed(seed, tenant_count=2, owner_only=False)
    factory = _factory(engine)

    async def scan():
        async with factory() as session:
            written = await remind_expiring_orders(session, now=NOW)
            await session.commit()
            return written

    # Two scans race for one due order; the scan guard admits one writer.
    results = await asyncio.gather(scan(), scan())
    assert sorted(results) == [0, 2]

    # A later scan finds both reminders already sent for this expiry.
    assert await scan() == 0

    async with factory() as session:
        rows = (await session.scalars(select(Notification))).all()
    assert len(rows) == 2
    assert {row.recipient_id for row in rows} == {user.id for user in users}
    assert {row.data["order_key"] for row in rows} == {order.id.hex}
    assert {row.data["expiry_epoch"] for row in rows} == {EXPIRY.timestamp()}

    async with factory() as session:
        for user in users:
            visible = await get_notifications(skip=0, limit=100, current_user=user, db=session)
            assert len(visible) == 1
            assert (await get_unread_count(current_user=user, db=session))["count"] == 1


@pytest.mark.asyncio
async def test_two_concurrent_claimers_send_once_with_frozen_key(pg_session, monkeypatch):
    engine, seed = pg_session
    org, users, order = await _seed(seed, tenant_count=1, owner_only=True)
    owner = users[0]
    factory = _factory(engine)

    async with factory() as session:
        assert await remind_expiring_orders(session, now=NOW) == 1
        await session.commit()
        notice = (await session.scalars(select(Notification))).one()
        notice_id = notice.id
        payload = notice.data["_email_payload"]
        assert notice.data["_email_status"] == "pending"

    # The provider blocks inside the uncertain-send window so a second claimer
    # runs after the first committed its lease but before the send completes.
    send_started = asyncio.Event()
    release_send = asyncio.Event()
    blocked_once = False

    async def gated_send(*_args):
        nonlocal blocked_once
        send_started.set()
        if not blocked_once:
            # Hold only the first delivery; a duplicate send must return at
            # once so a lease regression fails the assertion, never hangs.
            blocked_once = True
            await release_send.wait()
        return True

    send = AsyncMock(side_effect=gated_send)
    monkeypatch.setattr("app.services.order_expiry_reminders.send_order_expiry_email", send)

    async def claim():
        async with factory() as session:
            return await deliver_expiry_reminder_emails(session, now=NOW, batch_size=1)

    first = asyncio.create_task(claim())
    try:
        await asyncio.wait_for(send_started.wait(), timeout=10)
        # The first claimer already persisted its 15-minute lease, so the second
        # claimer must not send a duplicate for the same row.
        assert await claim() == 0
    finally:
        release_send.set()
    assert await asyncio.wait_for(first, timeout=10) == 1

    # One delivery, and it keeps the notification ID as the provider key.
    send.assert_awaited_once_with(payload, notice_id)

    async with factory() as session:
        terminal = await session.get(Notification, notice_id)
        assert terminal.data["_email_status"] == "sent"
        assert "_email_payload" not in terminal.data


@pytest.mark.asyncio
async def test_copied_leases_and_inbox_privacy(pg_session, monkeypatch):
    engine, seed = pg_session
    org, users, order = await _seed(seed, tenant_count=2, owner_only=True)
    owner, other = users
    factory = _factory(engine)
    active_payload = _frozen_payload(owner)

    async with factory() as session:
        active = _copied_row(
            owner,
            order,
            payload=active_payload,
            retry_at=(NOW + timedelta(minutes=15)).timestamp(),
            deadline=(NOW + timedelta(hours=20)).timestamp(),
        )
        expired = _copied_row(
            owner,
            order,
            payload=_frozen_payload(owner),
            retry_at=(NOW - timedelta(minutes=15)).timestamp(),
            deadline=(NOW + timedelta(hours=20)).timestamp(),
        )
        email_only = _copied_row(
            other,
            order,
            payload=_frozen_payload(other),
            retry_at=NOW.timestamp(),
            deadline=(NOW + timedelta(hours=20)).timestamp(),
        )
        null_data = Notification(
            id=uuid4(),
            recipient_id=owner.id,
            type=NotificationType.ORDER_UPDATE,
            title="No payload",
            message="Synthetic",
            data=None,
            created_at=NOW,
        )
        session.add_all([active, expired, email_only, null_data])
        await session.commit()
        active_id, expired_id = active.id, expired.id
        null_id = null_data.id

    send = AsyncMock(return_value=True)
    monkeypatch.setattr("app.services.order_expiry_reminders.send_order_expiry_email", send)

    # An active lease and a foreign recipient's copied row are not claimable.
    async with factory() as session:
        assert await deliver_expiry_reminder_emails(session, now=NOW) == 1
        assert send.await_count == 1

    # Only the expired lease was reclaimed, with its own frozen payload and ID.
    sent_payload, sent_id = send.await_args.args
    assert sent_id == expired_id
    assert sent_payload["to"] == [owner.email]

    # Past its lease the same active row is claimed with the frozen payload.
    async with factory() as session:
        assert await deliver_expiry_reminder_emails(session, now=NOW + timedelta(minutes=15)) == 1
    assert send.await_args.args == (active_payload, active_id)

    async with factory() as session:
        owner_rows = await get_notifications(skip=0, limit=100, current_user=owner, db=session)
        # The email-only copy is hidden; the NULL-payload row is visible.
        assert {row.id for row in owner_rows} == {null_id}
        assert (await get_unread_count(current_user=owner, db=session))["count"] == 1
        other_rows = await get_notifications(skip=0, limit=100, current_user=other, db=session)
        assert other_rows == []
        # Terminal rows never carry the private payload again.
        terminal = await session.get(Notification, expired_id)
        assert terminal.data["_email_status"] == "sent"
        assert "_email_payload" not in terminal.data

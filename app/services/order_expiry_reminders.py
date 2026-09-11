"""One in-app reminder per real order, recipient, and expiry time."""

from datetime import UTC, datetime, timedelta

from sqlalchemy import String, cast, exists, extract, func, or_, select, text
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.notification import Notification, NotificationType
from app.models.orderbook import OrderBookOrder, OrderBookStatus
from app.models.user import Organization, OrganizationProvenance, User, UserStatus

REMINDER_LEAD_TIME = timedelta(hours=48)
# Dedicated transaction lock prevents overlapping timer/manual runs from duplicating reminders.
REMINDER_LOCK_ID = 8246192048


async def remind_expiring_orders(
    db: AsyncSession,
    *,
    now: datetime | None = None,
    batch_size: int = 1000,
) -> int:
    if not 1 <= batch_size <= 10000:
        raise ValueError("batch_size must be between 1 and 10000")
    reference = now or datetime.now(UTC)
    if reference.tzinfo is None:
        raise ValueError("now must be timezone-aware")
    reference = reference.astimezone(UTC)
    if db.get_bind().dialect.name == "postgresql":
        acquired = await db.scalar(
            text("SELECT pg_try_advisory_xact_lock(:lock_id)"),
            {"lock_id": REMINDER_LOCK_ID},
        )
        if not acquired:
            return 0

    order = OrderBookOrder
    already_sent = exists(
        select(Notification.id).where(
            Notification.recipient_id == User.id,
            Notification.data["event"].as_string() == "order_expiring",
            Notification.data["order_key"].as_string()
            == func.replace(cast(order.id, String), "-", ""),
            Notification.data["expiry_epoch"].as_float()
            == extract("epoch", order.expires_at),
        )
    )
    rows = (
        await db.execute(
            select(
                order.id, order.side, order.expires_at, User.id.label("recipient_id")
            )
            .join(Organization, Organization.id == order.organization_id)
            .join(User, User.organization_id == order.organization_id)
            .where(
                order.provenance == OrganizationProvenance.REAL,
                Organization.provenance == OrganizationProvenance.REAL,
                User.status == UserStatus.APPROVED,
                or_(order.owner_user_id == User.id, order.owner_user_id.is_(None)),
                order.status.in_(
                    [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
                ),
                order.remaining_quantity_mt > 0,
                order.expires_at > reference,
                order.expires_at <= reference + REMINDER_LEAD_TIME,
                ~already_sent,
            )
            .order_by(order.expires_at, order.id, User.id)
            .limit(batch_size)
            .with_for_update(of=order)
        )
    ).all()
    for order in rows:
        expiry = (
            order.expires_at.replace(tzinfo=UTC)
            if order.expires_at.tzinfo is None
            else order.expires_at.astimezone(UTC)
        )
        db.add(
            Notification(
                recipient_id=order.recipient_id,
                type=NotificationType.ORDER_UPDATE,
                title="Order expires within 48 hours",
                message=f"Your {order.side.value} order {str(order.id)[:8]} expires on {expiry:%d %b %Y at %H:%M UTC}. Review it before expiry if you want to keep trading.",
                data={
                    "event": "order_expiring",
                    "order_id": str(order.id),
                    "order_key": order.id.hex,
                    "expiry_epoch": expiry.timestamp(),
                    "expires_at": expiry.isoformat(),
                },
                created_at=reference,
            )
        )
    await db.flush()
    return len(rows)

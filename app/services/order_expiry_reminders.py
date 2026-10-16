"""One in-app reminder per real order, recipient, and expiry time."""

from datetime import UTC, datetime, timedelta
from uuid import UUID

from sqlalchemy import String, and_, cast, exists, extract, func, or_, select, text
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.notification import Notification, NotificationType
from app.models.user_preference import UserPreference
from app.models.orderbook import OrderBookOrder, OrderBookStatus
from app.models.user import Organization, OrganizationProvenance, User, UserStatus

from app.services.email import build_order_expiry_email_payload, send_order_expiry_email

REMINDER_LEAD_TIME = timedelta(hours=48)
# Dedicated transaction lock prevents overlapping timer/manual runs from duplicating reminders.
REMINDER_LOCK_ID = 8246192048
EMAIL_RETRY_DELAY = timedelta(minutes=15)
# Resend retains idempotency keys for 24 hours. Stop sooner to avoid replaying an uncertain send.
# https://resend.com/docs/dashboard/emails/idempotency-keys
EMAIL_RETRY_WINDOW = timedelta(hours=23)


def _eligible_order_clauses(reference: datetime):
    order = OrderBookOrder
    return (
        order.provenance == OrganizationProvenance.REAL,
        Organization.provenance == OrganizationProvenance.REAL,
        User.status == UserStatus.APPROVED,
        or_(order.owner_user_id == User.id, order.owner_user_id.is_(None)),
        order.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        order.remaining_quantity_mt > 0,
        order.expires_at > reference,
        order.expires_at <= reference + REMINDER_LEAD_TIME,
    )


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
                order.id,
                order.side,
                order.expires_at,
                User.id.label("recipient_id"),
                User.email,
                User.email_verified,
                func.coalesce(
                    UserPreference.value["email_trade_updates"].as_boolean(), True
                ).label("email_enabled"),
                func.coalesce(
                    UserPreference.value["inapp_trade_updates"].as_boolean(), True
                ).label("inapp_enabled"),
            )
            .join(Organization, Organization.id == order.organization_id)
            .join(User, User.organization_id == order.organization_id)
            .outerjoin(
                UserPreference,
                and_(
                    UserPreference.user_id == User.id,
                    UserPreference.namespace == "notifications",
                ),
            )
            .where(
                *_eligible_order_clauses(reference),
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
        message = (
            f"Your {order.side.value} order {str(order.id)[:8]} expires on "
            f"{expiry:%d %b %Y at %H:%M UTC}. Review it before expiry if you want to keep trading."
        )
        db.add(
            Notification(
                recipient_id=order.recipient_id,
                type=NotificationType.ORDER_UPDATE,
                title="Order expires within 48 hours",
                message=message,
                data={
                    "event": "order_expiring",
                    "order_id": str(order.id),
                    "order_key": order.id.hex,
                    "expiry_epoch": expiry.timestamp(),
                    "expires_at": expiry.isoformat(),
                    "_email_only": not order.inapp_enabled,
                    "_email_status": "pending"
                    if order.email_verified and order.email_enabled
                    else "discarded",
                    "_email_payload": build_order_expiry_email_payload(
                        order.email, message
                    )
                    if order.email_verified and order.email_enabled
                    else None,
                    "_email_retry_at": reference.timestamp(),
                    "_email_deadline": min(
                        expiry, reference + EMAIL_RETRY_WINDOW
                    ).timestamp(),
                },
                created_at=reference,
            )
        )
    await db.flush()
    return len(rows)


def _finish_email(data: dict, status: str) -> dict:
    terminal = dict(data)
    terminal["_email_status"] = status
    for key in ("_email_payload", "_email_retry_at", "_email_deadline"):
        terminal.pop(key, None)
    return terminal


async def deliver_expiry_reminder_emails(
    db: AsyncSession,
    *,
    now: datetime | None = None,
    batch_size: int = 20,
) -> int:
    """Claim each email briefly, send without customer-row locks, then commit its result."""
    if not 1 <= batch_size <= 20:
        raise ValueError("email batch_size must be between 1 and 20")
    if now is not None and now.tzinfo is None:
        raise ValueError("now must be timezone-aware")
    sent = 0
    # ponytail: at most 80 emails/hour on the 15-minute timer; increase cadence if volume grows.
    for _ in range(batch_size):
        reference = (now or datetime.now(UTC)).astimezone(UTC)
        notice = await db.scalar(
            select(Notification)
            .where(
                Notification.data["event"].as_string() == "order_expiring",
                Notification.data["_email_status"].as_string() == "pending",
                Notification.data["_email_retry_at"].as_float()
                <= reference.timestamp(),
            )
            .order_by(Notification.created_at, Notification.id)
            .limit(1)
            .with_for_update(skip_locked=True)
        )
        if notice is None:
            await db.commit()
            break
        data = dict(notice.data)
        payload = data.get("_email_payload")
        if reference.timestamp() >= data["_email_deadline"] or not isinstance(
            payload, dict
        ):
            notice.data = _finish_email(data, "discarded")
            await db.commit()
            continue
        # Recheck current ownership, address, preferences and expiry before claiming delivery.
        email = await db.scalar(
            select(User.email)
            .select_from(User)
            .join(Organization, Organization.id == User.organization_id)
            .join(OrderBookOrder, OrderBookOrder.organization_id == Organization.id)
            .outerjoin(
                UserPreference,
                and_(
                    UserPreference.user_id == User.id,
                    UserPreference.namespace == "notifications",
                ),
            )
            .where(
                User.id == notice.recipient_id,
                User.email_verified.is_(True),
                func.coalesce(
                    UserPreference.value["email_trade_updates"].as_boolean(), True
                ).is_(True),
                OrderBookOrder.id == UUID(data["order_id"]),
                extract("epoch", OrderBookOrder.expires_at) == data["expiry_epoch"],
                *_eligible_order_clauses(reference),
            )
            .with_for_update(of=(User, OrderBookOrder))
        )
        if email is None or payload.get("to") != [email]:
            notice.data = _finish_email(data, "discarded")
            await db.commit()
            continue
        # This persisted lease survives a worker crash. Retries retain the same payload and key.
        notice_id = notice.id
        data["_email_retry_at"] = (reference + EMAIL_RETRY_DELAY).timestamp()
        notice.data = data
        await db.commit()
        accepted = await send_order_expiry_email(payload, notice_id)
        if accepted:
            notice = await db.get(
                Notification, notice_id, with_for_update=True, populate_existing=True
            )
            if notice is not None:
                notice.data = _finish_email(notice.data, "sent")
            await db.commit()
            sent += 1
    return sent

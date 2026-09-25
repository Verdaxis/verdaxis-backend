"""Bounded ingestion and admin timeline queries for per-user activity."""

from __future__ import annotations

import re
from datetime import UTC, datetime, timedelta
from typing import Literal
from uuid import UUID

from fastapi import HTTPException, status
from sqlalchemy import String, case, cast, exists, func, literal, select, union_all
from sqlalchemy.dialects.postgresql import insert as postgres_insert
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import noload

from app.market_catalog import (
    APPROVED_MARKET_PRODUCTS,
    DELIVERY_POINTS_BY_ID,
    DELIVERY_POINTS_BY_NAME,
)
from app.models.audit import AuditLog
from app.models.product_analytics import UserLoginDay
from app.models.user_activity import UserBrowsingEvent
from app.models.user import User
from app.models.user_preference import UserPreference
from app.models.watchlist import Watchlist, WatchlistTarget, WatchlistTargetType
from app.schemas.user_activity import BrowsingEventsIn, UserActivityItem, UserActivityPage
from app.services.audit_actions import (
    MARKET_WATCH_PREFERENCES_SAVED,
    WATCHLIST_PINNED,
    WATCHLIST_REMOVED_TARGET,
    WATCHLIST_SAVED_TARGET,
    WATCHLIST_UNPINNED,
)


ActivityKind = Literal["all", "browsing", "business", "login"]
MAX_BROWSING_EVENTS_PER_MINUTE = 300

_BUSINESS_ACTIONS = frozenset(
    {
        "order.created",
        "order.updated",
        "order.cancelled",
        "order.expired",
        "rfq.created",
        "rfq.cancelled",
        "rfq.quote_submitted",
        "rfq.quote_withdrawn",
        "trade.created",
        "trade.auto_matched",
        "trade.confirmed",
        "trade.declined",
        "trade.cancelled",
        "trade.delivered",
        "trade.paid",
        WATCHLIST_SAVED_TARGET,
        WATCHLIST_REMOVED_TARGET,
        WATCHLIST_PINNED,
        WATCHLIST_UNPINNED,
        MARKET_WATCH_PREFERENCES_SAVED,
    }
)

_SAFE_AUDIT_KEYS = frozenset(
    {
        "side",
        "product_id",
        "market_product",
        "market_product_code",
        "delivery_point_id",
        "availability_window",
        "availability_window_code",
        "quantity_mt",
        "price_per_mt_usd",
        "target_price_per_mt",
        "status",
        "order_id",
        "rfq_id",
        "products",
        "port_ids",
        "target_type",
    }
)
_SAFE_PORT_ID = re.compile(r"^[A-Za-z0-9_-]{1,64}$")


def _as_utc(value: datetime) -> datetime:
    return value.replace(tzinfo=UTC) if value.tzinfo is None else value.astimezone(UTC)


async def ingest_browsing_events(
    db: AsyncSession,
    *,
    user_id: UUID,
    payload: BrowsingEventsIn,
    received_at: datetime | None = None,
) -> int:
    """Insert a batch once per user/event UUID and return the accepted count."""
    now = received_at or datetime.now(UTC)
    unique_events = {event.id: event for event in payload.events}
    # Serialize the quota with other batches for this account. Authenticated
    # request paths already hold this lock; reacquiring it is transaction-local.
    await db.execute(select(User.id).where(User.id == user_id).with_for_update())
    existing_ids = set(
        (
            await db.execute(
                select(UserBrowsingEvent.event_id).where(
                    UserBrowsingEvent.user_id == user_id,
                    UserBrowsingEvent.event_id.in_(unique_events),
                )
            )
        ).scalars()
    )
    new_events = [
        event for event_id, event in unique_events.items() if event_id not in existing_ids
    ]
    recent_count = await db.scalar(
        select(func.count()).select_from(UserBrowsingEvent).where(
            UserBrowsingEvent.user_id == user_id,
            UserBrowsingEvent.received_at >= now - timedelta(minutes=1),
        )
    )
    if (recent_count or 0) + len(new_events) > MAX_BROWSING_EVENTS_PER_MINUTE:
        await db.rollback()
        raise HTTPException(
            status_code=status.HTTP_429_TOO_MANY_REQUESTS,
            detail="Activity event rate limit exceeded",
        )
    if not new_events:
        await db.commit()
        return 0
    rows = [
        {
            "user_id": user_id,
            "event_id": event.id,
            "consent_version": payload.consent_version,
            "action": event.action,
            "page": event.page,
            "market_product": event.market_product.value if event.market_product else None,
            "delivery_point_id": event.delivery_point_id,
            "availability_window": event.availability_window,
            "received_at": now,
        }
        for event in new_events
    ]
    dialect = db.bind.dialect.name if db.bind is not None else ""
    if dialect == "postgresql":
        statement = postgres_insert(UserBrowsingEvent).values(rows)
    elif dialect == "sqlite":
        statement = sqlite_insert(UserBrowsingEvent).values(rows)
    else:  # pragma: no cover - deployed and test databases use the branches above
        raise RuntimeError("user activity ingestion requires PostgreSQL or SQLite")
    statement = statement.on_conflict_do_nothing(
        index_elements=[UserBrowsingEvent.user_id, UserBrowsingEvent.event_id]
    ).returning(UserBrowsingEvent.event_id)
    accepted = len((await db.execute(statement)).scalars().all())
    await db.commit()
    return accepted


def _base_timeline_selects(*, user_id: UUID, cutoff: datetime, kind: ActivityKind):
    selects = []
    if kind in ("all", "browsing"):
        selects.append(
            select(
                cast(UserBrowsingEvent.event_id, String).label("row_id"),
                UserBrowsingEvent.received_at.label("occurred_at"),
                literal("browsing").label("source"),
                UserBrowsingEvent.action.label("action"),
                literal("browsing").label("variant"),
            ).where(
                UserBrowsingEvent.user_id == user_id,
                UserBrowsingEvent.received_at >= cutoff,
            )
        )

    if kind in ("all", "login"):
        selects.append(
            select(
                cast(UserLoginDay.id, String).label("row_id"),
                UserLoginDay.last_login_at.label("occurred_at"),
                literal("login").label("source"),
                literal("login_day").label("action"),
                literal("login").label("variant"),
            ).where(
                UserLoginDay.user_id == user_id,
                UserLoginDay.last_login_at >= cutoff,
            )
        )

    if kind in ("all", "business"):
        selects.append(
            select(
                cast(AuditLog.id, String).label("row_id"),
                AuditLog.timestamp.label("occurred_at"),
                literal("business").label("source"),
                AuditLog.action.label("action"),
                literal("audit").label("variant"),
            ).where(
                AuditLog.user_id == user_id,
                AuditLog.action.in_(_BUSINESS_ACTIONS),
                AuditLog.timestamp >= cutoff,
            )
        )

        target_audit_exists = exists(
            select(literal(1)).where(
                AuditLog.user_id == user_id,
                func.replace(AuditLog.resource_id, "-", "")
                == func.replace(cast(WatchlistTarget.id, String), "-", ""),
                AuditLog.action.in_((WATCHLIST_SAVED_TARGET, WATCHLIST_PINNED)),
            )
        )
        selects.append(
            select(
                cast(WatchlistTarget.id, String).label("row_id"),
                WatchlistTarget.created_at.label("occurred_at"),
                literal("business").label("source"),
                case(
                    (WatchlistTarget.target_type == WatchlistTargetType.PIN, WATCHLIST_PINNED),
                    else_=WATCHLIST_SAVED_TARGET,
                ).label("action"),
                literal("watchlist_snapshot").label("variant"),
            )
            .join(Watchlist, Watchlist.id == WatchlistTarget.watchlist_id)
            .where(
                Watchlist.user_id == user_id,
                WatchlistTarget.created_at >= cutoff,
                ~target_audit_exists,
            )
        )

        preference_audit_exists = exists(
            select(literal(1)).where(
                AuditLog.user_id == user_id,
                func.replace(AuditLog.resource_id, "-", "")
                == func.replace(cast(UserPreference.id, String), "-", ""),
                AuditLog.action == MARKET_WATCH_PREFERENCES_SAVED,
            )
        )
        selects.append(
            select(
                cast(UserPreference.id, String).label("row_id"),
                UserPreference.updated_at.label("occurred_at"),
                literal("business").label("source"),
                literal(MARKET_WATCH_PREFERENCES_SAVED).label("action"),
                literal("preference_snapshot").label("variant"),
            ).where(
                UserPreference.user_id == user_id,
                UserPreference.namespace == "market_watch",
                UserPreference.updated_at >= cutoff,
                ~preference_audit_exists,
            )
        )
    return selects


def _safe_audit_details(entry: AuditLog) -> dict:
    details = {"resource_type": entry.resource_type}
    if entry.resource_id:
        details["resource_id"] = entry.resource_id
    changes = entry.changes if isinstance(entry.changes, dict) else {}
    for key in _SAFE_AUDIT_KEYS:
        value = changes.get(key)
        if value is None or isinstance(value, (str, int, float, bool)):
            if key in changes:
                details[key] = value
        elif isinstance(value, list) and all(
            isinstance(item, str) and len(item) <= 64 for item in value
        ):
            details[key] = value
    delivery_point_id = details.get("delivery_point_id")
    if isinstance(delivery_point_id, str):
        try:
            point = DELIVERY_POINTS_BY_ID.get(UUID(delivery_point_id))
        except ValueError:
            point = None
        if point:
            details["delivery_point_name"] = point.name
    if entry.action == MARKET_WATCH_PREFERENCES_SAVED:
        details["port_names"] = _port_names(details.get("port_ids", []))
    return details


def _port_names(port_ids: object) -> list[str]:
    if not isinstance(port_ids, list):
        return []
    names = []
    for value in port_ids:
        if not isinstance(value, str):
            continue
        point = DELIVERY_POINTS_BY_NAME.get(value)
        if point is None:
            try:
                point = DELIVERY_POINTS_BY_ID.get(UUID(value))
            except ValueError:
                point = None
        if point is not None and point.name not in names:
            names.append(point.name)
    return names


def _safe_market_watch_details(value: object) -> dict:
    stored = value if isinstance(value, dict) else {}
    products = [
        item
        for item in stored.get("products", [])
        if isinstance(item, str) and item in APPROVED_MARKET_PRODUCTS
    ]
    port_ids = [
        item
        for item in stored.get("portIds", [])
        if isinstance(item, str) and _SAFE_PORT_ID.fullmatch(item)
    ]
    return {
        "products": products,
        "port_ids": port_ids,
        "port_names": _port_names(port_ids),
        "current_state": True,
    }


async def get_user_activity_page(
    db: AsyncSession,
    *,
    user_id: UUID,
    days: int,
    kind: ActivityKind,
    limit: int,
    offset: int,
    now: datetime | None = None,
) -> UserActivityPage:
    cutoff = (now or datetime.now(UTC)) - timedelta(days=days)
    combined = union_all(
        *_base_timeline_selects(user_id=user_id, cutoff=cutoff, kind=kind)
    ).subquery("user_activity")
    result = await db.execute(
        select(combined)
        .order_by(
            combined.c.occurred_at.desc(),
            combined.c.source.asc(),
            combined.c.row_id.asc(),
        )
        .limit(limit + 1)
        .offset(offset)
    )
    rows = result.mappings().all()
    visible = rows[:limit]

    ids_by_variant: dict[str, list[UUID]] = {}
    for row in visible:
        ids_by_variant.setdefault(row["variant"], []).append(UUID(row["row_id"]))

    loaded: dict[str, dict[UUID, object]] = {}
    model_by_variant = {
        "browsing": (UserBrowsingEvent, UserBrowsingEvent.event_id),
        "login": (UserLoginDay, UserLoginDay.id),
        "audit": (AuditLog, AuditLog.id),
        "watchlist_snapshot": (WatchlistTarget, WatchlistTarget.id),
        "preference_snapshot": (UserPreference, UserPreference.id),
    }
    for variant, ids in ids_by_variant.items():
        model, id_column = model_by_variant[variant]
        statement = select(model).where(id_column.in_(ids))
        if variant == "browsing":
            statement = statement.where(UserBrowsingEvent.user_id == user_id)
        elif variant == "watchlist_snapshot":
            statement = statement.options(
                noload(WatchlistTarget.delivery_point),
                noload(WatchlistTarget.order),
            )
        records = (await db.execute(statement)).scalars().all()
        key_name = "event_id" if variant == "browsing" else "id"
        loaded[variant] = {getattr(record, key_name): record for record in records}

    items = []
    for row in visible:
        record_id = UUID(row["row_id"])
        record = loaded[row["variant"]].get(record_id)
        if record is None:
            # A prune or user mutation may commit between the page query and
            # bounded detail hydration under READ COMMITTED.
            continue
        if row["variant"] == "browsing":
            details = {"page": record.page}
            for field in ("market_product", "delivery_point_id", "availability_window"):
                value = getattr(record, field)
                if value is not None:
                    details[field] = str(value)
            if record.delivery_point_id:
                point = DELIVERY_POINTS_BY_ID.get(record.delivery_point_id)
                if point:
                    details["delivery_point_name"] = point.name
        elif row["variant"] == "login":
            details = {"login_count": record.login_count}
        elif row["variant"] == "audit":
            details = _safe_audit_details(record)
        elif row["variant"] == "watchlist_snapshot":
            details = {
                "target_type": record.target_type.value,
                "market_product": record.market_product_code,
                "delivery_point_id": str(record.delivery_point_id),
                "availability_window": record.availability_window_code,
                "current_state": True,
            }
            if record.delivery_point_id:
                point = DELIVERY_POINTS_BY_ID.get(record.delivery_point_id)
                if point:
                    details["delivery_point_name"] = point.name
                elif (
                    isinstance(record.snapshot_delivery_point_name, str)
                    and len(record.snapshot_delivery_point_name) <= 120
                ):
                    details["delivery_point_name"] = record.snapshot_delivery_point_name
            if record.order_id:
                details["order_id"] = str(record.order_id)
        else:
            details = _safe_market_watch_details(record.value)
        items.append(
            UserActivityItem(
                id=f"{row['variant']}:{row['row_id']}",
                occurred_at=_as_utc(row["occurred_at"]),
                source=row["source"],
                action=row["action"],
                details=details,
            )
        )

    last_activity_at = await db.scalar(select(func.max(combined.c.occurred_at)))
    return UserActivityPage(
        items=items,
        has_more=len(rows) > limit,
        last_activity_at=_as_utc(last_activity_at) if last_activity_at else None,
    )

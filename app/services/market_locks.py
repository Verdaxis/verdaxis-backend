"""Transaction-scoped serialization for one canonical market slice."""
from __future__ import annotations

import hashlib
from uuid import UUID
from collections.abc import Iterable
from inspect import isawaitable

from fastapi import HTTPException
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderSide
from app.services.availability_windows import normalize_availability_window
from app.services.market_transactions import (
    RetryableMarketTransactionError,
    database_sqlstate,
)

MarketSliceKey = tuple[OrderSide | str, UUID, UUID | None, str]
MARKET_LOCK_TIMEOUT = "500ms"
MARKET_STATEMENT_TIMEOUT = "5s"


def _lock_failure(exc: DBAPIError) -> HTTPException | None:
    sqlstate = database_sqlstate(exc)
    if sqlstate == "55P03":
        return HTTPException(status_code=409, detail="Market slice is busy; retry the request")
    if sqlstate == "57014":
        return HTTPException(status_code=503, detail="Market slice lock timed out; retry the request")
    return None


def market_slice_lock_sort_key(key: MarketSliceKey) -> tuple[str, str, str]:
    """Return the canonical order used before any market row lock is taken."""
    _, product_id, delivery_point_id, availability_window = key
    return (str(product_id), str(delivery_point_id or ""), normalize_availability_window(availability_window))


def market_slice_lock_key(
    *,
    side: OrderSide | str,
    product_id: UUID,
    delivery_point_id: UUID | None,
    availability_window: str,
) -> int:
    # BID and ASK share one executable slice; both sides must serialize on the
    # same transaction-scoped lock.
    canonical = "|".join(
        (str(product_id), str(delivery_point_id or ""), normalize_availability_window(availability_window))
    )
    raw = hashlib.sha256(canonical.encode("utf-8")).digest()[:8]
    value = int.from_bytes(raw, byteorder="big", signed=True)
    return value or 1


async def _acquire_advisory_lock(
    db: AsyncSession,
    *,
    key: int,
    busy_detail: str,
) -> None:
    try:
        await db.execute(text(f"SET LOCAL lock_timeout = '{MARKET_LOCK_TIMEOUT}'"))
        await db.execute(text(f"SET LOCAL statement_timeout = '{MARKET_STATEMENT_TIMEOUT}'"))
        await db.execute(text("SELECT pg_advisory_xact_lock(:lock_key)"), {"lock_key": key})
    except DBAPIError as exc:
        sqlstate = database_sqlstate(exc)
        if sqlstate in {"40001", "40P01"}:
            raise RetryableMarketTransactionError(sqlstate) from exc
        failure = _lock_failure(exc)
        if failure is not None:
            await db.rollback()
            if failure.status_code == 409:
                failure.detail = busy_detail
            raise failure from exc
        raise


async def acquire_market_slice_lock(
    db: AsyncSession,
    *,
    side: OrderSide | str,
    product_id: UUID,
    delivery_point_id: UUID | None,
    availability_window: str,
) -> None:
    """Acquire a PostgreSQL transaction lock; deliberately no-op under SQLite."""
    bind = db.get_bind()
    # Lightweight unit doubles sometimes expose get_bind as an AsyncMock;
    # there is no real database lock to acquire in that case.
    if isawaitable(bind):
        close = getattr(bind, "close", None)
        if close is not None:
            close()
        return
    if bind is None or bind.dialect.name == "sqlite":
        return
    key = market_slice_lock_key(
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window=availability_window,
    )
    # These are transaction-local safety bounds. Runtime-wide timeout policy
    # remains a deployment concern; market operations never wait indefinitely.
    await _acquire_advisory_lock(
        db,
        key=key,
        busy_detail="Market slice is busy; retry the request",
    )


async def acquire_market_slice_locks(db: AsyncSession, keys: Iterable[MarketSliceKey]) -> None:
    """Acquire every affected slice lock in one deterministic order."""
    # The advisory hash intentionally ignores side because BID and ASK share
    # one executable slice. Deduplicate on that actual identity as well.
    unique = {
        (key[1], key[2], normalize_availability_window(key[3]))
        for key in keys
    }
    for product_id, delivery_point_id, availability_window in sorted(
        unique,
        key=lambda key: (str(key[0]), str(key[1] or ""), key[2]),
    ):
        await acquire_market_slice_lock(
            db,
            side=OrderSide.BID,
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            availability_window=availability_window,
        )

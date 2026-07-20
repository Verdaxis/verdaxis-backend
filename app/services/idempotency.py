"""Transaction-scoped idempotency locks."""
from __future__ import annotations

import hashlib
import json
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession

from app.services.market_transactions import (
    RetryableMarketTransactionError,
    database_sqlstate,
)


IDEMPOTENCY_LOCK_TIMEOUT = "500ms"
IDEMPOTENCY_STATEMENT_TIMEOUT = "5s"
ORDER_CREATE_OPERATION = "order.create"
TRADE_CREATE_OPERATION = "trade.create"
INVENTORY_PUBLISH_OPERATION = "inventory.publish"


def idempotency_request_hash(payload: object) -> str:
    encoded = json.dumps(
        payload,
        default=str,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def idempotency_lock_key(tenant_id: UUID, operation: str, key: str) -> int:
    digest = hashlib.sha256(f"{tenant_id}|{operation}|{key}".encode()).digest()[:8]
    return int.from_bytes(digest, byteorder="big", signed=True) or 1


async def acquire_idempotency_lock(
    db: AsyncSession,
    *,
    tenant_id: UUID,
    operation: str,
    key: str,
) -> None:
    bind = db.get_bind()
    if bind is None or bind.dialect.name == "sqlite":
        return
    try:
        await db.execute(text(f"SET LOCAL lock_timeout = '{IDEMPOTENCY_LOCK_TIMEOUT}'"))
        await db.execute(text(f"SET LOCAL statement_timeout = '{IDEMPOTENCY_STATEMENT_TIMEOUT}'"))
        await db.execute(
            text("SELECT pg_advisory_xact_lock(:lock_key)"),
            {"lock_key": idempotency_lock_key(tenant_id, operation, key)},
        )
    except DBAPIError as exc:
        sqlstate = database_sqlstate(exc)
        if sqlstate in {"40001", "40P01"}:
            raise RetryableMarketTransactionError(sqlstate) from exc
        if sqlstate in {"55P03", "57014"}:
            # PostgreSQL aborts the transaction after these failures. Roll it
            # back here so dependency cleanup and callers see a stable state.
            await db.rollback()
            if sqlstate == "55P03":
                raise HTTPException(
                    status_code=409,
                    detail="Idempotency key is busy; retry the request",
                ) from exc
            raise HTTPException(
                status_code=503,
                detail="Idempotency lock timed out; retry the request",
            ) from exc
        raise

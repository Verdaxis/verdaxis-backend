"""Atomic replay for the six supported market lifecycle commands."""
from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, date, datetime
from decimal import Decimal
from enum import Enum
import hashlib
import json
from typing import Any
from uuid import UUID

from fastapi import HTTPException, Request, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.command_result import MarketCommandResult
from app.services.idempotency import acquire_idempotency_lock


TRADE_CONFIRM_OPERATION = "trade.confirm"
TRADE_DECLINE_OPERATION = "trade.decline"
TRADE_DELIVER_OPERATION = "trade.deliver"
TRADE_PAY_OPERATION = "trade.pay"
ORDER_AMEND_OPERATION = "order.amend"
ORDER_CANCEL_OPERATION = "order.cancel"


def _canonical_value(value: Any) -> Any:
    if isinstance(value, Decimal):
        return format(value.normalize(), "f")
    if isinstance(value, UUID):
        return str(value)
    if isinstance(value, Enum):
        return _canonical_value(value.value)
    if isinstance(value, datetime):
        if value.tzinfo is None or value.utcoffset() is None:
            raise ValueError("Command datetimes must include a timezone")
        return value.astimezone(UTC).isoformat().replace("+00:00", "Z")
    if isinstance(value, date):
        return value.isoformat()
    if isinstance(value, dict):
        return {str(key): _canonical_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_canonical_value(item) for item in value]
    return value


def command_request_hash(payload: object) -> str:
    encoded = json.dumps(
        _canonical_value(payload), allow_nan=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


@dataclass(frozen=True)
class CommandAttempt:
    actor_user_id: UUID
    effective_organization_id: UUID
    support_context_id: UUID | None
    operation: str
    idempotency_key: str
    request_hash: str
    resource_type: str
    resource_id: UUID
    replay: MarketCommandResult | None = None


async def prepare_command_attempt(
    db: AsyncSession,
    request: Request,
    *,
    actor_user_id: UUID,
    effective_organization_id: UUID | None,
    support_context_id: UUID | None,
    operation: str,
    resource_type: str,
    resource_id: UUID,
    payload: object,
) -> CommandAttempt | None:
    raw_key = request.headers.get("Idempotency-Key")
    if raw_key is None:
        return None
    key = raw_key.strip()
    if not key or len(key) > 255:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Idempotency-Key must be 1-255 characters")
    if effective_organization_id is None:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Market organization is required")

    request_hash = command_request_hash(payload)
    await acquire_idempotency_lock(db, lock_scope_id=actor_user_id, operation=operation, key=key)
    replay = (
        await db.execute(
            select(MarketCommandResult).where(
                MarketCommandResult.actor_user_id == actor_user_id,
                MarketCommandResult.operation == operation,
                MarketCommandResult.idempotency_key == key,
            )
        )
    ).scalar_one_or_none()
    attempt = CommandAttempt(
        actor_user_id=actor_user_id,
        effective_organization_id=effective_organization_id,
        support_context_id=support_context_id,
        operation=operation,
        idempotency_key=key,
        request_hash=request_hash,
        resource_type=resource_type,
        resource_id=resource_id,
        replay=replay,
    )
    if replay is not None and (
        replay.request_hash != attempt.request_hash
        or replay.effective_organization_id != attempt.effective_organization_id
        or replay.support_context_id != attempt.support_context_id
        or replay.resource_type != attempt.resource_type
        or replay.resource_id != attempt.resource_id
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Idempotency-Key was reused with a different request or actor context",
        )
    return attempt


async def record_command_success(
    db: AsyncSession,
    attempt: CommandAttempt | None,
    *,
    response_status: int,
    response_body: dict | list | None,
) -> None:
    if attempt is None:
        return
    if attempt.replay is not None:
        raise ValueError("A replay cannot record a second command result")
    db.add(
        MarketCommandResult(
            actor_user_id=attempt.actor_user_id,
            effective_organization_id=attempt.effective_organization_id,
            support_context_id=attempt.support_context_id,
            operation=attempt.operation,
            idempotency_key=attempt.idempotency_key,
            request_hash=attempt.request_hash,
            resource_type=attempt.resource_type,
            resource_id=attempt.resource_id,
            response_status=response_status,
            response_body=response_body,
        )
    )
    await db.flush()

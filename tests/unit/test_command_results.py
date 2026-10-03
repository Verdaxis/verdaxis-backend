from datetime import UTC, datetime
from decimal import Decimal
from enum import Enum
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import HTTPException
from starlette.requests import Request

from app.services import command_results
from app.services.command_results import command_request_hash, prepare_command_attempt


class _Value(str, Enum):
    VALUE = "VALUE"


def _request(key: str = "retry-key") -> Request:
    return Request(
        {
            "type": "http",
            "method": "PUT",
            "path": "/test",
            "headers": [(b"idempotency-key", key.encode())],
        }
    )


def test_command_hash_canonicalizes_economic_and_identity_values():
    identity = uuid4()
    instant = datetime(2026, 10, 3, 12, tzinfo=UTC)

    left = {
        "amount": Decimal("10.00"),
        "identity": identity,
        "value": _Value.VALUE,
        "instant": instant,
    }
    right = {
        "instant": instant.astimezone(UTC),
        "value": "VALUE",
        "identity": str(identity),
        "amount": Decimal("10"),
    }

    assert command_request_hash(left) == command_request_hash(right)
    assert command_request_hash({"value": None}) != command_request_hash({})



@pytest.mark.asyncio
async def test_command_lock_contention_is_retryable_unknown_outcome(monkeypatch):
    async def busy(*args, **kwargs):
        raise HTTPException(status_code=409, detail="Idempotency key is busy; retry the request")

    monkeypatch.setattr(command_results, "acquire_idempotency_lock", busy)
    actor_id = uuid4()
    organization_id = uuid4()

    with pytest.raises(HTTPException) as caught:
        await prepare_command_attempt(
            SimpleNamespace(),
            _request(),
            actor_user_id=actor_id,
            effective_organization_id=organization_id,
            support_context_id=None,
            operation=command_results.TRADE_CONFIRM_OPERATION,
            resource_type="trade",
            resource_id=uuid4(),
            payload={"operation": command_results.TRADE_CONFIRM_OPERATION},
        )

    assert caught.value.status_code == 503
    assert caught.value.detail == "Idempotency key is busy; retry the same request"

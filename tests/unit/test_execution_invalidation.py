import inspect
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi import HTTPException
from sqlalchemy.exc import DBAPIError

from app.models.orderbook import OrderBookStatus
from app.services import execution_invalidation


def test_security_invalidation_has_no_bulk_update_escape_hatch():
    source = inspect.getsource(execution_invalidation.invalidate_execution_state)

    assert "execute(update(" not in source
    assert "with_for_update(nowait=True)" in inspect.getsource(execution_invalidation)


def test_unfilled_inventory_reservation_is_released_exactly_once():
    inventory = SimpleNamespace(reserved_stock_mt=Decimal("10.00"))
    order = SimpleNamespace(
        status=OrderBookStatus.PARTIALLY_FILLED,
        remaining_quantity_mt=Decimal("4.00"),
        inventory_item_id="inventory-id",
    )

    released = execution_invalidation.release_unfilled_inventory_reservation(order, inventory)

    assert released == Decimal("4.00")
    assert inventory.reserved_stock_mt == Decimal("6.00")
    order.status = OrderBookStatus.CANCELLED
    assert execution_invalidation.release_unfilled_inventory_reservation(order, inventory) == Decimal("0.00")
    assert inventory.reserved_stock_mt == Decimal("6.00")


@pytest.mark.asyncio
async def test_publication_happens_only_when_caller_invokes_post_commit_publisher(monkeypatch):
    publish = AsyncMock()
    monkeypatch.setattr(execution_invalidation.event_bus, "publish", publish)
    result = execution_invalidation.InvalidationResult()
    result.events.append(("orderbook", "order_cancelled", {"id": "order-id"}))

    assert publish.await_count == 0
    await execution_invalidation.publish_execution_invalidation(result)
    publish.assert_awaited_once_with("orderbook", "order_cancelled", {"id": "order-id"})


class _LockNotAvailable(Exception):
    sqlstate = "55P03"


@pytest.mark.asyncio
async def test_request_invalidation_contention_rolls_back_with_one_retryable_contract(monkeypatch):
    db = AsyncMock()
    monkeypatch.setattr(
        execution_invalidation,
        "invalidate_execution_state",
        AsyncMock(side_effect=DBAPIError("statement", {}, _LockNotAvailable())),
    )

    with pytest.raises(HTTPException) as exc_info:
        await execution_invalidation.invalidate_execution_state_for_request(
            db,
            user_ids=["user-id"],
        )

    assert exc_info.value.status_code == 409
    assert exc_info.value.headers == {"Retry-After": "1"}
    assert exc_info.value.detail == {
        "code": "EXECUTION_INVALIDATION_BUSY",
        "message": "Execution state is being updated; retry this request",
        "retry_after_seconds": 1,
    }
    db.rollback.assert_awaited_once()


@pytest.mark.asyncio
async def test_request_invalidation_does_not_translate_unexpected_database_errors(monkeypatch):
    db = AsyncMock()
    failure = DBAPIError("statement", {}, RuntimeError("unexpected"))
    monkeypatch.setattr(
        execution_invalidation,
        "invalidate_execution_state",
        AsyncMock(side_effect=failure),
    )

    with pytest.raises(DBAPIError) as exc_info:
        await execution_invalidation.invalidate_execution_state_for_request(db, user_ids=["user-id"])

    assert exc_info.value is failure
    db.rollback.assert_not_awaited()

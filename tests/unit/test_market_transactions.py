from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi import HTTPException


@pytest.mark.asyncio
async def test_market_transaction_retries_one_genuine_deadlock_from_fresh_transaction():
    from app.services.market_transactions import (
        RetryableMarketTransactionError,
        retry_market_transaction,
    )

    db = AsyncMock()
    user = SimpleNamespace(id=None)
    calls = 0

    @retry_market_transaction(max_attempts=2)
    async def operation(*, db, current_user):
        nonlocal calls
        calls += 1
        if calls == 1:
            raise RetryableMarketTransactionError("40P01")
        return "committed"

    assert await operation(db=db, current_user=user) == "committed"
    assert calls == 2
    db.rollback.assert_awaited_once()


@pytest.mark.asyncio
async def test_market_transaction_stops_after_bound_with_stable_503():
    from app.services.market_transactions import (
        RetryableMarketTransactionError,
        retry_market_transaction,
    )

    db = AsyncMock()
    calls = 0

    @retry_market_transaction(max_attempts=2)
    async def operation(*, db):
        nonlocal calls
        calls += 1
        raise RetryableMarketTransactionError("40001")

    with pytest.raises(HTTPException) as rejected:
        await operation(db=db)

    assert rejected.value.status_code == 503
    assert rejected.value.detail == "Market transaction serialization failed; retry the request"
    assert calls == 2
    assert db.rollback.await_count == 2


@pytest.mark.asyncio
async def test_market_transaction_does_not_retry_application_conflicts():
    from app.services.market_transactions import retry_market_transaction

    db = AsyncMock()
    calls = 0

    @retry_market_transaction(max_attempts=2)
    async def operation(*, db):
        nonlocal calls
        calls += 1
        raise HTTPException(status_code=409, detail="Order slice changed")

    with pytest.raises(HTTPException, match="Order slice changed"):
        await operation(db=db)

    assert calls == 1
    db.rollback.assert_not_awaited()

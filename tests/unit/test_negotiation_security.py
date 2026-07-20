from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request

from app.models.negotiation import NegotiationStatus
from app.models.orderbook import Trade
from app.models.user import UserRole
from app.routers import negotiations


def _request() -> Request:
    return Request(
        {
            "type": "http",
            "method": "POST",
            "path": "/api/negotiations/example/accept",
            "query_string": b"",
            "headers": [],
            "client": ("127.0.0.1", 1234),
            "scheme": "https",
            "server": ("test", 443),
        }
    )


def _negotiation():
    buyer_org_id = uuid4()
    seller_org_id = uuid4()
    buyer_user_id = uuid4()
    seller_user_id = uuid4()
    return SimpleNamespace(
        id=uuid4(),
        bid_order_id=None,
        ask_order_id=uuid4(),
        initiator_org_id=buyer_org_id,
        counterparty_org_id=seller_org_id,
        initiator_user_id=buyer_user_id,
        counterparty_user_id=seller_user_id,
        accepted_by_user_id=None,
        initiator_side="BUYER",
        product_id=uuid4(),
        quantity_mt=10,
        current_price=100,
        status=NegotiationStatus.OPEN,
        last_actor_org_id=buyer_org_id,
        trade_id=None,
        expires_at=datetime.now(UTC) + timedelta(hours=1),
        rounds=[],
    )


def test_same_org_colleague_is_not_a_concrete_negotiation_party():
    neg = _negotiation()
    colleague = SimpleNamespace(id=uuid4(), organization_id=neg.counterparty_org_id)

    with pytest.raises(HTTPException) as exc_info:
        negotiations._assert_concrete_party(neg, colleague)

    assert exc_info.value.status_code == 404


@pytest.mark.asyncio
async def test_acceptance_is_disabled_fail_closed():
    """Negotiation execution is disabled in this release (market-owned decision).

    Until the separately reviewed bilateral contract ships, acceptance must
    never create a trade, consume capacity, or mutate the negotiation — it
    fails closed with 409 for every party.
    """
    neg = _negotiation()
    seller = SimpleNamespace(
        id=neg.counterparty_user_id,
        organization_id=neg.counterparty_org_id,
        role=UserRole.SUPPLIER,
    )
    db = AsyncMock()
    db.add = MagicMock()

    accept = negotiations.accept_negotiation
    while hasattr(accept, "__wrapped__"):
        accept = accept.__wrapped__

    with pytest.raises(HTTPException) as exc_info:
        await accept(
            request=_request(),
            negotiation_id=neg.id,
            db=db,
            current_user=seller,
            _security_admission=None,
        )

    assert exc_info.value.status_code == 409
    assert not any(
        isinstance(call.args[0], Trade) for call in db.add.call_args_list
    ), "disabled acceptance must never create a trade"
    assert neg.status == NegotiationStatus.OPEN
    assert neg.accepted_by_user_id is None
    db.commit.assert_not_awaited()

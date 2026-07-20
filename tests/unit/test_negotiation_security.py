from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request

from app.models.negotiation import NegotiationStatus
from app.models.orderbook import Trade
from app.models.orderbook import OrderBookStatus
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


def test_acceptance_consumes_every_locked_canonical_order_capacity():
    consume = getattr(negotiations, "_consume_locked_order_capacity", None)
    assert callable(consume), "negotiation acceptance capacity consumption is missing"
    neg = _negotiation()
    bid = SimpleNamespace(
        id=uuid4(),
        remaining_quantity_mt=15,
        status=OrderBookStatus.OPEN,
    )
    ask = SimpleNamespace(
        id=neg.ask_order_id,
        remaining_quantity_mt=10,
        status=OrderBookStatus.OPEN,
    )
    neg.bid_order_id = bid.id

    consume(neg, {bid.id: bid, ask.id: ask})

    assert bid.remaining_quantity_mt == 5
    assert bid.status == OrderBookStatus.PARTIALLY_FILLED
    assert ask.remaining_quantity_mt == 0
    assert ask.status == OrderBookStatus.FILLED


@pytest.mark.asyncio
async def test_acceptance_persists_both_user_provenance_fields_on_trade():
    neg = _negotiation()
    seller = SimpleNamespace(
        id=neg.counterparty_user_id,
        organization_id=neg.counterparty_org_id,
        role=UserRole.SUPPLIER,
    )
    db = AsyncMock()
    db.add = MagicMock()

    def assign_trade_id(value):
        if isinstance(value, Trade) and value.id is None:
            value.id = uuid4()

    db.add.side_effect = assign_trade_id
    response_marker = SimpleNamespace(id=neg.id)
    locked_ask = SimpleNamespace(
        id=neg.ask_order_id,
        remaining_quantity_mt=neg.quantity_mt,
        status=OrderBookStatus.OPEN,
    )

    with (
        patch.object(negotiations, "_load_negotiation", new=AsyncMock(return_value=neg)),
        patch.object(negotiations, "_revalidate_negotiation_parties", new=AsyncMock()),
        patch.object(
            negotiations,
            "_load_negotiation_orders",
            new=AsyncMock(return_value={locked_ask.id: locked_ask}),
        ),
        patch.object(negotiations, "_notify_org_users", new=AsyncMock()),
        patch.object(negotiations, "_batch_org_names", new=AsyncMock(return_value={})),
        patch.object(negotiations, "record_audit", new=AsyncMock()),
        patch.object(negotiations.event_bus, "publish", new=AsyncMock()),
        patch.object(negotiations, "publish_trade_event", new=AsyncMock()),
        patch.object(negotiations, "trade_created_event", return_value={}),
        patch.object(negotiations, "track_analytics_event"),
        patch.object(negotiations, "_build_response", new=AsyncMock(return_value=response_marker)),
    ):
        result = await negotiations.accept_negotiation.__wrapped__(
            request=_request(),
            negotiation_id=neg.id,
            db=db,
            current_user=seller,
        )

    created_trade = next(
        call.args[0]
        for call in db.add.call_args_list
        if isinstance(call.args[0], Trade)
    )
    assert created_trade.buyer_id == neg.initiator_org_id
    assert created_trade.seller_id == neg.counterparty_org_id
    assert created_trade.buyer_user_id == neg.initiator_user_id
    assert created_trade.seller_user_id == neg.counterparty_user_id
    assert neg.accepted_by_user_id == seller.id
    assert neg.status == NegotiationStatus.AGREED
    assert locked_ask.remaining_quantity_mt == 0
    assert locked_ask.status == OrderBookStatus.FILLED
    assert result is response_marker

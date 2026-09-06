from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import HTTPException

from app.models.orderbook import OrderCreationMethod, OrderSide
from app.models.user import UserRole
from app.routers.trades import _assisted_trade_side, _delivery_amounts


def _assisted_fixture(side: OrderSide):
    order_id = uuid4()
    organization_id = uuid4()
    owner_id = uuid4()
    order = SimpleNamespace(
        id=order_id,
        side=side,
        organization_id=organization_id,
        owner_user_id=owner_id,
        created_by_actor_user_id=owner_id,
        creation_method=OrderCreationMethod.MARKET_SUPPORT,
    )
    trade = SimpleNamespace(
        ask_order_id=order_id if side == OrderSide.ASK else None,
        bid_order_id=order_id if side == OrderSide.BID else None,
        seller_id=organization_id if side == OrderSide.ASK else uuid4(),
        buyer_id=organization_id if side == OrderSide.BID else uuid4(),
        seller_user_id=owner_id if side == OrderSide.ASK else uuid4(),
        buyer_user_id=owner_id if side == OrderSide.BID else uuid4(),
    )
    return trade, order, organization_id, owner_id


@pytest.mark.parametrize(
    ("side", "role"),
    [(OrderSide.ASK, UserRole.SUPPLIER), (OrderSide.BID, UserRole.BUYER)],
)
def test_assisted_trade_side_keeps_recorded_owner_and_economic_side(side, role):
    trade, order, organization_id, owner_id = _assisted_fixture(side)

    assert _assisted_trade_side(trade, order) == (
        side,
        organization_id,
        role,
        owner_id,
    )


def test_assisted_trade_side_rejects_moved_economic_party():
    trade, order, _, _ = _assisted_fixture(OrderSide.ASK)
    trade.seller_id = uuid4()

    with pytest.raises(HTTPException) as error:
        _assisted_trade_side(trade, order)
    assert error.value.status_code == 403


def test_delivery_amounts_round_total_then_commission_half_up():
    assert _delivery_amounts(
        Decimal("1.23"), Decimal("100.01"), Decimal("0.5")
    ) == (Decimal("123.01"), Decimal("0.62"))
    assert _delivery_amounts(
        Decimal("1.00"), Decimal("1.00"), Decimal("0.5")
    ) == (Decimal("1.00"), Decimal("0.01"))

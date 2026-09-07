"""Tests for anonymous trade serialization."""

from datetime import UTC, datetime
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from pydantic import ValidationError


def _trade(status):
    from app.models.orderbook import Initiator, Trade
    from app.models.user import OrganizationProvenance

    buyer_id, seller_id = uuid4(), uuid4()
    trade = Trade(
        id=uuid4(),
        buyer_id=buyer_id,
        seller_id=seller_id,
        initiator_org_id=buyer_id,
        buyer_provenance=OrganizationProvenance.REAL,
        seller_provenance=OrganizationProvenance.REAL,
        initiated_by=Initiator.BUYER,
        is_anonymous=True,
        quantity_mt=Decimal("100"),
        price_per_mt_usd=Decimal("500"),
        status=status,
        created_at=datetime.now(UTC),
    )
    trade.__dict__["buyer"] = SimpleNamespace(name="Buyer Org")
    trade.__dict__["seller"] = SimpleNamespace(name="Seller Org")
    return trade, buyer_id, seller_id


def test_trade_model_has_is_anonymous():
    from app.models.orderbook import Trade
    assert hasattr(Trade, "is_anonymous"), "Trade ORM model must have is_anonymous column"


def test_trade_schema_has_is_anonymous():
    from app.schemas.orderbook import TradeResponse
    assert "is_anonymous" in TradeResponse.model_fields
    assert not TradeResponse.model_fields["buyer_id"].is_required()
    assert not TradeResponse.model_fields["seller_id"].is_required()


def test_trade_schema_defaults_is_anonymous_to_false():
    from app.schemas.orderbook import TradeResponse
    assert TradeResponse.model_fields["is_anonymous"].default is False


def test_order_create_schema_has_is_anonymous():
    from app.schemas.orderbook import OrderCreate
    assert "is_anonymous" in OrderCreate.model_fields


def test_order_create_schema_defaults_is_anonymous_to_true():
    from app.schemas.orderbook import OrderCreate
    assert OrderCreate.model_fields["is_anonymous"].default is True


def test_order_create_schema_rejects_non_anonymous_orders():
    from app.schemas.orderbook import OrderCreate, OrderSide

    with pytest.raises(ValidationError):
        OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("100"),
            price_per_mt_usd=Decimal("500"),
            is_anonymous=False,
        )


def test_anonymous_pending_trade_shows_only_buyers_own_identity():
    from app.models.orderbook import TradeStatus
    from app.routers.trades import build_trade_response

    trade, buyer_id, seller_id = _trade(TradeStatus.PENDING_CONFIRMATION)

    buyer_view = build_trade_response(trade, viewer_org_id=buyer_id)
    seller_view = build_trade_response(trade, viewer_org_id=seller_id)

    assert (buyer_view.buyer_id, buyer_view.buyer_name) == (buyer_id, "Buyer Org")
    assert (buyer_view.seller_id, buyer_view.seller_name) == (None, "Anonymous")
    assert (seller_view.buyer_id, seller_view.buyer_name) == (None, "Anonymous")
    assert (seller_view.seller_id, seller_view.seller_name) == (seller_id, "Seller Org")


def test_anonymous_declined_and_cancelled_trades_remain_redacted():
    from app.models.orderbook import TradeStatus
    from app.routers.trades import build_trade_response

    for status in (TradeStatus.DECLINED, TradeStatus.CANCELLED):
        trade, _buyer_id, _seller_id = _trade(status)
        response = build_trade_response(trade, viewer_org_id=uuid4())
        assert response.buyer_id is None
        assert response.seller_id is None
        assert response.buyer_name == response.seller_name == "Anonymous"


def test_anonymous_confirmed_trade_reveals_both_parties_to_participant():
    from app.models.orderbook import TradeStatus
    from app.routers.trades import build_trade_response

    for status in (TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID):
        trade, buyer_id, seller_id = _trade(status)
        response = build_trade_response(trade, viewer_org_id=buyer_id)
        assert (response.buyer_id, response.buyer_name) == (buyer_id, "Buyer Org")
        assert (response.seller_id, response.seller_name) == (seller_id, "Seller Org")


def test_anonymous_trade_without_or_with_wrong_viewer_fails_closed():
    from app.models.orderbook import TradeStatus
    from app.routers.trades import build_trade_response

    trade, _buyer_id, _seller_id = _trade(TradeStatus.CONFIRMED)
    for viewer_org_id in (None, uuid4()):
        response = build_trade_response(trade, viewer_org_id=viewer_org_id)
        assert response.buyer_id is None
        assert response.seller_id is None
        assert response.buyer_name == response.seller_name == "Anonymous"

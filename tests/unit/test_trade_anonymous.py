"""Tests for Trade anonymization flag."""


def test_trade_model_has_is_anonymous():
    from app.models.orderbook import Trade
    assert hasattr(Trade, "is_anonymous"), "Trade ORM model must have is_anonymous column"


def test_trade_schema_has_is_anonymous():
    from app.schemas.orderbook import TradeResponse
    assert "is_anonymous" in TradeResponse.model_fields


def test_trade_schema_defaults_is_anonymous_to_false():
    from app.schemas.orderbook import TradeResponse
    assert TradeResponse.model_fields["is_anonymous"].default is False


def test_order_create_schema_has_is_anonymous():
    from app.schemas.orderbook import OrderCreate
    assert "is_anonymous" in OrderCreate.model_fields


def test_order_create_schema_defaults_is_anonymous_to_true():
    from app.schemas.orderbook import OrderCreate
    assert OrderCreate.model_fields["is_anonymous"].default is True

import pytest
from app.models.orderbook import OrderType, OrderBookStatus, OrderBookOrder
import app.schemas.orderbook as schema_module


# ============ Model-layer enum tests ============

def test_order_type_enum_values():
    assert OrderType.MARKET.value == "MARKET"
    assert OrderType.LIMIT.value == "LIMIT"
    assert OrderType.STOP.value == "STOP"
    assert OrderType.STOP_LIMIT.value == "STOP_LIMIT"
    assert OrderType.AON.value == "AON"
    assert OrderType.OCO.value == "OCO"


def test_triggered_status_exists():
    assert OrderBookStatus.TRIGGERED.value == "TRIGGERED"


def test_orderbook_order_has_order_type_field():
    assert hasattr(OrderBookOrder, 'order_type')


def test_orderbook_order_has_stop_price_field():
    assert hasattr(OrderBookOrder, 'stop_price')


def test_orderbook_order_has_linked_order_id_field():
    assert hasattr(OrderBookOrder, 'linked_order_id')


# ============ Schema-layer enum tests ============

def test_schema_order_type_enum_exists():
    assert hasattr(schema_module, 'OrderType')


def test_schema_order_type_enum_values():
    SchemaOrderType = schema_module.OrderType
    assert SchemaOrderType.MARKET.value == "MARKET"
    assert SchemaOrderType.LIMIT.value == "LIMIT"
    assert SchemaOrderType.STOP.value == "STOP"
    assert SchemaOrderType.STOP_LIMIT.value == "STOP_LIMIT"
    assert SchemaOrderType.AON.value == "AON"
    assert SchemaOrderType.OCO.value == "OCO"


def test_schema_triggered_status_exists():
    assert schema_module.OrderBookStatus.TRIGGERED.value == "TRIGGERED"


def test_order_create_has_order_type_field():
    OrderCreate = schema_module.OrderCreate
    assert 'order_type' in OrderCreate.model_fields


def test_order_create_has_stop_price_field():
    OrderCreate = schema_module.OrderCreate
    assert 'stop_price' in OrderCreate.model_fields


def test_order_create_has_linked_order_id_field():
    OrderCreate = schema_module.OrderCreate
    assert 'linked_order_id' in OrderCreate.model_fields


def test_order_response_has_order_type_field():
    OrderResponse = schema_module.OrderResponse
    assert 'order_type' in OrderResponse.model_fields


def test_order_create_stop_price_optional_for_limit():
    """LIMIT orders can be created without stop_price."""
    from decimal import Decimal
    order = schema_module.OrderCreate(
        side=schema_module.OrderSide.BID,
        fuel_type="VLSFO",
        region="SE Asia",
        quantity_mt=Decimal("100"),
        price_per_mt_usd=Decimal("600"),
        order_type=schema_module.OrderType.LIMIT,
    )
    assert order.stop_price is None


def test_order_create_stop_price_required_for_stop():
    """STOP orders must include stop_price; validation should catch omission."""
    from decimal import Decimal
    from pydantic import ValidationError
    with pytest.raises(ValidationError, match="stop_price"):
        schema_module.OrderCreate(
            side=schema_module.OrderSide.BID,
            fuel_type="VLSFO",
            region="SE Asia",
            quantity_mt=Decimal("100"),
            price_per_mt_usd=Decimal("600"),
            order_type=schema_module.OrderType.STOP,
            # stop_price intentionally omitted
        )


def test_order_create_stop_price_required_for_stop_limit():
    """STOP_LIMIT orders must include stop_price."""
    from decimal import Decimal
    from pydantic import ValidationError
    with pytest.raises(ValidationError, match="stop_price"):
        schema_module.OrderCreate(
            side=schema_module.OrderSide.BID,
            fuel_type="VLSFO",
            region="SE Asia",
            quantity_mt=Decimal("100"),
            price_per_mt_usd=Decimal("600"),
            order_type=schema_module.OrderType.STOP_LIMIT,
            # stop_price intentionally omitted
        )


def test_order_create_valid_stop_order():
    """A well-formed STOP order passes validation."""
    from decimal import Decimal
    order = schema_module.OrderCreate(
        side=schema_module.OrderSide.ASK,
        fuel_type="VLSFO",
        region="SE Asia",
        quantity_mt=Decimal("500"),
        price_per_mt_usd=Decimal("610"),
        order_type=schema_module.OrderType.STOP,
        stop_price=Decimal("605"),
    )
    assert order.stop_price == Decimal("605")


def test_order_create_default_is_limit():
    """When order_type is omitted the default is LIMIT (preserves legacy behavior)."""
    from decimal import Decimal
    order = schema_module.OrderCreate(
        side=schema_module.OrderSide.BID,
        fuel_type="VLSFO",
        region="SE Asia",
        quantity_mt=Decimal("100"),
        price_per_mt_usd=Decimal("600"),
    )
    assert order.order_type == schema_module.OrderType.LIMIT

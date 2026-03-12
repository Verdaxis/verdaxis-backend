"""Unit tests for price discovery Pydantic schemas."""
import pytest
from decimal import Decimal
from datetime import datetime, date
from uuid import uuid4

from app.schemas.orderbook import PriceSummary, PriceDiscoveryResponse


class TestPriceSummary:
    def test_valid_price_summary(self):
        product_id = uuid4()
        dp_id = uuid4()
        ps = PriceSummary(
            product_id=product_id,
            product_name="Methanol Green",
            fuel_type="Methanol",
            delivery_point_id=dp_id,
            delivery_point_name="Singapore",
            region="Asia",
            last_price=Decimal("540.00"),
            avg_price_24h=Decimal("538.50"),
            high_24h=Decimal("545.00"),
            low_24h=Decimal("532.00"),
            volume_24h=Decimal("12500"),
            trade_count_24h=8,
            price_change_pct=Decimal("1.25"),
            last_trade_at=datetime(2026, 2, 12, 10, 30, 0),
        )
        assert ps.fuel_type == "Methanol"
        assert ps.product_id == product_id
        assert ps.delivery_point_id == dp_id
        assert ps.product_name == "Methanol Green"
        assert ps.trade_count_24h == 8
        assert ps.price_change_pct == Decimal("1.25")

    def test_defaults_for_no_trades(self):
        ps = PriceSummary(
            fuel_type="Ammonia",
            region="Middle East",
            last_price=None,
            avg_price_24h=None,
            high_24h=None,
            low_24h=None,
            volume_24h=Decimal("0"),
            trade_count_24h=0,
            price_change_pct=None,
            last_trade_at=None,
        )
        assert ps.last_price is None
        assert ps.trade_count_24h == 0
        assert ps.product_id is None  # optional
        assert ps.delivery_point_id is None  # optional


class TestPriceDiscoveryResponse:
    def test_wraps_summaries(self):
        summary = PriceSummary(
            product_id=uuid4(),
            product_name="Methanol Green",
            fuel_type="Methanol",
            delivery_point_id=uuid4(),
            delivery_point_name="Singapore",
            region="Asia",
            last_price=Decimal("540"),
            avg_price_24h=Decimal("538"),
            high_24h=Decimal("545"),
            low_24h=Decimal("532"),
            volume_24h=Decimal("12500"),
            trade_count_24h=8,
            price_change_pct=Decimal("1.25"),
            last_trade_at=datetime(2026, 2, 12, 10, 30),
        )
        resp = PriceDiscoveryResponse(
            summaries=[summary],
            generated_at=datetime(2026, 2, 12, 10, 31),
        )
        assert len(resp.summaries) == 1
        assert resp.generated_at is not None

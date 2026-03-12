"""
Unit tests for the price discovery router logic.
Tests the aggregation query builder without a live DB by mocking the session.
"""
import pytest
from unittest.mock import AsyncMock, MagicMock
from datetime import date, datetime
from decimal import Decimal
from uuid import uuid4

from app.routers.price_discovery import aggregate_trade_prices, compute_reference_prices
from app.schemas.orderbook import ReferencePriceItem


class TestAggregateFunction:
    """Test the pure aggregation logic."""

    @pytest.mark.asyncio
    async def test_returns_empty_list_when_no_trades(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        summaries = await aggregate_trade_prices(mock_db)
        assert summaries == []

    @pytest.mark.asyncio
    async def test_filters_by_fuel_type(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await aggregate_trade_prices(mock_db, fuel_type="Methanol")
        # Verify query execution occurred.
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_filters_by_region(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await aggregate_trade_prices(mock_db, region="Singapore")
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_filters_by_product_id(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        pid = uuid4()
        await aggregate_trade_prices(mock_db, product_id=pid)
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_filters_by_delivery_point_id(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        dpid = uuid4()
        await aggregate_trade_prices(mock_db, delivery_point_id=dpid)
        assert mock_db.execute.call_count >= 1


class TestComputeReferencePrices:
    """Test the VWAP reference price computation."""

    @pytest.mark.asyncio
    async def test_returns_empty_list_when_no_trades(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)
        assert prices == []

    @pytest.mark.asyncio
    async def test_filters_by_fuel_type(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(mock_db, fuel_type="Methanol")
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_filters_by_region(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(mock_db, region="ARA")
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_filters_by_date_range(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(
            mock_db,
            date_from=date(2026, 1, 1),
            date_to=date(2026, 3, 1),
        )
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_filters_by_product_id(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        pid = uuid4()
        await compute_reference_prices(mock_db, product_id=pid)
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_computes_vwap_correctly(self):
        """VWAP = sum(price * qty) / sum(qty)."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        # Simulate: 2 trades for Methanol/Singapore on 2026-02-15
        # Trade 1: 100 MT @ $500 = $50,000
        # Trade 2: 200 MT @ $550 = $110,000
        # VWAP = $160,000 / 300 = $533.33
        pid = uuid4()
        dpid = uuid4()
        mock_row = MagicMock()
        mock_row.product_id = pid
        mock_row.product_name = "Methanol Green"
        mock_row.fuel_type = "Methanol"
        mock_row.delivery_point_id = dpid
        mock_row.delivery_point_name = "Singapore"
        mock_row.region = "Asia"
        mock_row.trade_date = date(2026, 2, 15)
        mock_row.weighted_sum = Decimal("160000.00")
        mock_row.total_volume = Decimal("300.00")
        mock_row.trade_count = 2
        mock_result.all.return_value = [mock_row]
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)

        assert len(prices) == 1
        assert prices[0].product_id == pid
        assert prices[0].product_name == "Methanol Green"
        assert prices[0].fuel_type == "Methanol"
        assert prices[0].delivery_point_id == dpid
        assert prices[0].delivery_point_name == "Singapore"
        assert prices[0].region == "Asia"
        assert prices[0].vwap_usd == Decimal("533.33")
        assert prices[0].total_volume_mt == Decimal("300.00")
        assert prices[0].trade_count == 2
        assert prices[0].date == date(2026, 2, 15)

    @pytest.mark.asyncio
    async def test_multiple_markets_returned(self):
        """Multiple product/delivery_point/date groups are returned."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        pid1 = uuid4()
        pid2 = uuid4()
        dpid1 = uuid4()
        dpid2 = uuid4()

        row1 = MagicMock()
        row1.product_id = pid1
        row1.product_name = "Methanol Green"
        row1.fuel_type = "Methanol"
        row1.delivery_point_id = dpid1
        row1.delivery_point_name = "Singapore"
        row1.region = "Asia"
        row1.trade_date = date(2026, 2, 15)
        row1.weighted_sum = Decimal("100000.00")
        row1.total_volume = Decimal("200.00")
        row1.trade_count = 3

        row2 = MagicMock()
        row2.product_id = pid2
        row2.product_name = "Ammonia Green"
        row2.fuel_type = "Ammonia"
        row2.delivery_point_id = dpid2
        row2.delivery_point_name = "ARA"
        row2.region = "Europe"
        row2.trade_date = date(2026, 2, 15)
        row2.weighted_sum = Decimal("75000.00")
        row2.total_volume = Decimal("100.00")
        row2.trade_count = 1

        mock_result.all.return_value = [row1, row2]
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)

        assert len(prices) == 2
        assert prices[0].fuel_type == "Methanol"
        assert prices[0].vwap_usd == Decimal("500.00")
        assert prices[1].fuel_type == "Ammonia"
        assert prices[1].vwap_usd == Decimal("750.00")

    @pytest.mark.asyncio
    async def test_zero_volume_returns_zero_vwap(self):
        """Edge case: if total_volume is 0 (should not normally happen), VWAP is 0."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_row = MagicMock()
        mock_row.product_id = uuid4()
        mock_row.product_name = "LNG Conventional"
        mock_row.fuel_type = "LNG"
        mock_row.delivery_point_id = uuid4()
        mock_row.delivery_point_name = "Tokyo"
        mock_row.region = "Asia"
        mock_row.trade_date = date(2026, 3, 1)
        mock_row.weighted_sum = Decimal("0")
        mock_row.total_volume = Decimal("0")
        mock_row.trade_count = 0
        mock_result.all.return_value = [mock_row]
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)

        assert len(prices) == 1
        assert prices[0].vwap_usd == Decimal("0")
        assert prices[0].total_volume_mt == Decimal("0")

    @pytest.mark.asyncio
    async def test_combined_filters(self):
        """All filters can be used together."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(
            mock_db,
            date_from=date(2026, 1, 1),
            date_to=date(2026, 2, 28),
            fuel_type="Methanol",
            region="ARA",
            product_id=uuid4(),
            delivery_point_id=uuid4(),
        )
        assert mock_db.execute.call_count == 1


class TestReferencePriceItemSchema:
    """Test the Pydantic schema for reference price items."""

    def test_valid_schema(self):
        pid = uuid4()
        dpid = uuid4()
        item = ReferencePriceItem(
            product_id=pid,
            product_name="Methanol Green",
            fuel_type="Methanol",
            delivery_point_id=dpid,
            delivery_point_name="Singapore",
            region="Asia",
            vwap_usd=Decimal("533.33"),
            total_volume_mt=Decimal("300.00"),
            trade_count=2,
            date=date(2026, 2, 15),
        )
        assert item.product_id == pid
        assert item.fuel_type == "Methanol"
        assert item.vwap_usd == Decimal("533.33")
        assert item.date == date(2026, 2, 15)

    def test_schema_serialization(self):
        item = ReferencePriceItem(
            product_id=uuid4(),
            product_name="Ammonia Green",
            fuel_type="Ammonia",
            delivery_point_id=uuid4(),
            delivery_point_name="ARA",
            region="Europe",
            vwap_usd=Decimal("750.00"),
            total_volume_mt=Decimal("100.00"),
            trade_count=1,
            date=date(2026, 3, 1),
        )
        data = item.model_dump()
        assert data["fuel_type"] == "Ammonia"
        assert data["trade_count"] == 1
        assert data["product_name"] == "Ammonia Green"


def test_reference_price_item_has_visibility_field():
    from app.schemas.orderbook import ReferencePriceItem
    from decimal import Decimal
    from datetime import date
    item = ReferencePriceItem(
        fuel_type="Methanol", region="Europe",
        vwap_usd=Decimal("525.50"), total_volume_mt=Decimal("5000"),
        trade_count=3, date=date(2026, 3, 12), visibility="internal",
    )
    assert item.visibility == "internal"


def test_reference_price_item_defaults_to_external():
    from app.schemas.orderbook import ReferencePriceItem
    from decimal import Decimal
    from datetime import date
    item = ReferencePriceItem(
        fuel_type="Methanol", region="Europe",
        vwap_usd=Decimal("525.50"), total_volume_mt=Decimal("5000"),
        trade_count=3, date=date(2026, 3, 12),
    )
    assert item.visibility == "external"


def test_get_reference_prices_accepts_visibility_param():
    import inspect
    from app.routers.price_discovery import get_reference_prices
    assert "visibility" in inspect.signature(get_reference_prices).parameters

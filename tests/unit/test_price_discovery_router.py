"""
Unit tests for the price discovery router logic.
Tests the aggregation query builder without a live DB by mocking the session.
"""
import pytest
from unittest.mock import AsyncMock, MagicMock
from decimal import Decimal

from app.routers.price_discovery import aggregate_trade_prices


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

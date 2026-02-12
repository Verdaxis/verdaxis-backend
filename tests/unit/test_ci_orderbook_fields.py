"""Test that OrderBookOrder has CI-related columns."""
import pytest
from app.models.orderbook import OrderBookOrder


class TestOrderBookOrderCIFields:
    def test_has_carbon_intensity_column(self):
        assert hasattr(OrderBookOrder, 'carbon_intensity_gco2_mj')

    def test_has_energy_density_column(self):
        assert hasattr(OrderBookOrder, 'energy_density_mj_kg')

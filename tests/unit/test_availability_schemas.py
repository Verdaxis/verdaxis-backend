"""Test availability schemas."""
import pytest
from decimal import Decimal

from app.schemas.availability import PortFuelAvailability, AvailabilityLevel


class TestPortFuelAvailability:
    def test_green_availability(self):
        a = PortFuelAvailability(
            port_id="sg-sin",
            port_name="Singapore",
            lat=1.29,
            lng=103.85,
            fuel_type="Methanol",
            total_stock_mt=Decimal("5000"),
            supplier_count=3,
            availability_level=AvailabilityLevel.AVAILABLE,
            avg_price_per_mt=Decimal("540"),
        )
        assert a.availability_level == AvailabilityLevel.AVAILABLE

    def test_red_availability(self):
        a = PortFuelAvailability(
            port_id="ae-fuj",
            port_name="Fujairah",
            lat=25.12,
            lng=56.33,
            fuel_type="Methanol",
            total_stock_mt=Decimal("0"),
            supplier_count=0,
            availability_level=AvailabilityLevel.NONE,
            avg_price_per_mt=None,
        )
        assert a.availability_level == AvailabilityLevel.NONE

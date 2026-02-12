"""Test demand signal schemas."""
import pytest
from decimal import Decimal
from datetime import datetime

from app.schemas.demand import DemandSignal, UrgencyLevel


class TestDemandSignal:
    def test_high_urgency_spot(self):
        d = DemandSignal(
            fuel_type="Methanol",
            region="Singapore",
            volume_mt=Decimal("2000"),
            max_price_per_mt=Decimal("560"),
            urgency=UrgencyLevel.HIGH,
            bid_count=3,
            earliest_delivery="Spot",
            created_at=datetime(2026, 2, 12),
        )
        assert d.urgency == UrgencyLevel.HIGH
        assert d.bid_count == 3

    def test_low_urgency_forward(self):
        d = DemandSignal(
            fuel_type="Biofuel",
            region="ARA",
            volume_mt=Decimal("5000"),
            max_price_per_mt=Decimal("1200"),
            urgency=UrgencyLevel.LOW,
            bid_count=1,
            earliest_delivery="Forward 2027",
            created_at=datetime(2026, 2, 12),
        )
        assert d.urgency == UrgencyLevel.LOW

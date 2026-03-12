"""Test the enriched OrderResponse with CI pricing."""
import pytest
from decimal import Decimal
from uuid import uuid4
from datetime import datetime

from app.schemas.orderbook import OrderResponseWithCI, CIAdjustedPrice


class TestOrderResponseWithCI:
    def test_has_ci_price_field(self):
        ci = CIAdjustedPrice(
            base_price_per_mt=Decimal("540"),
            carbon_intensity_gco2_mj=Decimal("15"),
            fueleu_ghg_intensity=Decimal("91"),
            compliance_cost_per_mt=Decimal("-105.77"),
            effective_price_per_mt=Decimal("434.23"),
            ghg_reduction_pct=Decimal("83.5"),
        )
        resp = OrderResponseWithCI(
            id=uuid4(),
            side="ASK",
            product_id=uuid4(),
            product_name="Methanol Green",
            fuel_type="Methanol",
            fuel_grade="Green",
            region="Singapore",
            quantity_mt=Decimal("1000"),
            remaining_quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("540"),
            availability_window="Spot",
            is_verdaxis_verified=True,
            status="OPEN",
            created_at=datetime.utcnow(),
            ci_adjusted_price=ci,
        )
        assert resp.ci_adjusted_price is not None
        assert resp.ci_adjusted_price.effective_price_per_mt == Decimal("434.23")

    def test_ci_price_optional(self):
        resp = OrderResponseWithCI(
            id=uuid4(),
            side="BID",
            product_id=uuid4(),
            product_name="LNG Conventional",
            fuel_type="LNG",
            fuel_grade="Conventional",
            region="Houston",
            quantity_mt=Decimal("500"),
            remaining_quantity_mt=Decimal("500"),
            price_per_mt_usd=Decimal("680"),
            availability_window="Spot",
            is_verdaxis_verified=False,
            status="OPEN",
            created_at=datetime.utcnow(),
            ci_adjusted_price=None,
        )
        assert resp.ci_adjusted_price is None

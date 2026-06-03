"""Unit tests for public trade tape endpoint — live status and anonymization."""

import uuid
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from unittest.mock import MagicMock

import pytest

from app.routers.trade_tape import _is_market_hours, _build_tape_entry
from app.schemas.trade_tape import TradeTapeEntry, TradeTapeResponse


# ---------------------------------------------------------------------------
# _is_market_hours
# ---------------------------------------------------------------------------

class TestIsMarketHours:
    @pytest.mark.parametrize("hour", [0, 7, 8, 12, 17, 18, 23])
    def test_market_has_no_session_delay(self, hour):
        dt = datetime(2026, 3, 15, hour, 0, 0, tzinfo=timezone.utc)
        assert _is_market_hours(dt) is True


# ---------------------------------------------------------------------------
# _build_tape_entry — anonymization + field mapping
# ---------------------------------------------------------------------------

def _make_mock_trade(
    trade_id: uuid.UUID | None = None,
    quantity: Decimal = Decimal("500.00"),
    price: Decimal = Decimal("1200.00"),
    confirmed_at: datetime | None = None,
    market_product: str | None = "BIO_METHANOL",
    fuel_type: str = "VLSFO",
    fuel_grade: str = "Conventional",
    region: str = "Amsterdam",
    availability_window: str = "SPOT",
) -> MagicMock:
    """Build a mock Trade with nested order relationships."""
    trade_id = trade_id or uuid.uuid4()
    trade = MagicMock()
    trade.id = trade_id
    trade.quantity_mt = quantity
    trade.price_per_mt_usd = price
    trade.confirmed_at = confirmed_at or datetime.now(timezone.utc)
    trade.buyer_id = uuid.uuid4()
    trade.seller_id = uuid.uuid4()

    # Mock the related order
    order = MagicMock()
    order.market_product = market_product
    order.fuel_type = fuel_type
    order.fuel_grade = fuel_grade
    order.region = "Europe" if region in {"Amsterdam", "Rotterdam", "Antwerp"} else "Asia"
    order.delivery_point_name = region
    order.availability_window = availability_window

    trade.ask_order = order
    trade.bid_order = None

    return trade


class TestBuildTapeEntry:
    def test_id_is_shortened_to_8_chars(self):
        known_id = uuid.UUID("12345678-1234-5678-1234-567812345678")
        trade = _make_mock_trade(trade_id=known_id)
        entry = _build_tape_entry(trade)
        assert entry.id == "12345678"
        assert len(entry.id) == 8

    def test_no_buyer_seller_info_in_entry(self):
        trade = _make_mock_trade()
        entry = _build_tape_entry(trade)
        entry_dict = entry.model_dump()
        assert "buyer" not in entry_dict
        assert "seller" not in entry_dict
        assert "buyer_id" not in entry_dict
        assert "seller_id" not in entry_dict
        assert "buyer_name" not in entry_dict
        assert "seller_name" not in entry_dict

    def test_total_usd_calculated(self):
        trade = _make_mock_trade(quantity=Decimal("100.00"), price=Decimal("500.00"))
        entry = _build_tape_entry(trade)
        assert entry.total_usd == Decimal("50000.00")

    def test_fuel_type_from_order(self):
        trade = _make_mock_trade(fuel_type="HSFO")
        entry = _build_tape_entry(trade)
        assert entry.fuel_type == "HSFO"

    def test_market_product_from_order(self):
        trade = _make_mock_trade(market_product="E_METHANOL")
        entry = _build_tape_entry(trade)
        assert entry.market_product == "E_METHANOL"

    def test_region_from_order(self):
        trade = _make_mock_trade(region="Singapore")
        entry = _build_tape_entry(trade)
        assert entry.region == "Singapore"

    def test_fuel_grade_from_order(self):
        trade = _make_mock_trade(fuel_grade="Green")
        entry = _build_tape_entry(trade)
        assert entry.fuel_grade == "Green"

    def test_availability_window_from_order(self):
        trade = _make_mock_trade(availability_window="Q2 2026")
        entry = _build_tape_entry(trade)
        assert entry.availability_window == "2026-Q2"

    def test_no_order_fallback_empty_strings(self):
        trade = _make_mock_trade()
        trade.ask_order = None
        trade.bid_order = None
        entry = _build_tape_entry(trade)
        assert entry.fuel_type == ""
        assert entry.fuel_grade == ""
        assert entry.region == ""
        assert entry.availability_window == ""

    def test_bid_order_used_when_no_ask_order(self):
        trade = _make_mock_trade(fuel_type="MGO")
        # Move the ask_order to bid_order
        trade.bid_order = trade.ask_order
        trade.ask_order = None
        entry = _build_tape_entry(trade)
        assert entry.fuel_type == "MGO"

    def test_demo_trade_flagged_without_party_identity(self, monkeypatch):
        trade = _make_mock_trade()
        monkeypatch.setattr(
            "app.routers.trade_tape.is_demo_market_organization",
            lambda organization_id: organization_id in {trade.buyer_id, trade.seller_id},
        )
        entry = _build_tape_entry(trade)
        assert entry.is_demo_trade is True


# ---------------------------------------------------------------------------
# Schema validation
# ---------------------------------------------------------------------------

class TestTradeTapeSchemas:
    def test_tape_entry_schema_fields(self):
        expected = {
            "id", "market_product", "fuel_type", "fuel_grade", "region",
            "quantity_mt", "price_per_mt_usd", "total_usd",
            "confirmed_at", "availability_window", "is_demo_trade",
        }
        assert set(TradeTapeEntry.model_fields.keys()) == expected

    def test_tape_response_schema_fields(self):
        expected = {"items", "total", "market_hours"}
        assert set(TradeTapeResponse.model_fields.keys()) == expected

    def test_tape_entry_no_party_fields(self):
        """Ensure the schema itself has no buyer/seller fields."""
        fields = set(TradeTapeEntry.model_fields.keys())
        forbidden = {"buyer_id", "seller_id", "buyer_name", "seller_name", "organization_id"}
        assert fields.isdisjoint(forbidden), f"Found forbidden fields: {fields & forbidden}"

    def test_tape_response_round_trip(self):
        entry = TradeTapeEntry(
            id="abcd1234",
            market_product="BIO_METHANOL",
            fuel_type="VLSFO",
            fuel_grade="Conventional",
            region="Amsterdam",
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("750.50"),
            total_usd=Decimal("750500.00"),
            confirmed_at=datetime(2026, 3, 15, 10, 0, 0, tzinfo=timezone.utc),
            availability_window="SPOT",
        )
        resp = TradeTapeResponse(items=[entry], total=1, market_hours=True)
        assert resp.total == 1
        assert resp.market_hours is True
        assert len(resp.items) == 1
        assert resp.items[0].id == "abcd1234"

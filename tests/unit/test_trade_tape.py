"""Unit tests for public trade tape endpoint — live status and anonymization."""

import uuid
from datetime import datetime, timezone
from decimal import Decimal
from unittest.mock import MagicMock, AsyncMock

import pytest

from app.routers.trade_tape import _is_market_hours, _build_tape_entry, get_trade_tape
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
    product_id: uuid.UUID | None = None,
    delivery_point_id: uuid.UUID | None = None,
    availability_window: str = "SPOT",
) -> MagicMock:
    """Build a mock Trade with immutable market snapshots."""
    trade_id = trade_id or uuid.uuid4()
    trade = MagicMock()
    trade.id = trade_id
    trade.quantity_mt = quantity
    trade.price_per_mt_usd = price
    trade.confirmed_at = confirmed_at or datetime.now(timezone.utc)
    trade.buyer_id = uuid.uuid4()
    trade.seller_id = uuid.uuid4()
    trade.buyer_provenance = "REAL"
    trade.seller_provenance = "REAL"
    trade.product_id = product_id or uuid.uuid4()
    trade.market_product = market_product
    trade.fuel_type = fuel_type
    trade.fuel_grade = fuel_grade
    trade.delivery_point_id = delivery_point_id or uuid.uuid4()
    trade.delivery_point_name = region
    trade.delivery_point_region = "Europe" if region in {"Amsterdam", "Rotterdam", "Antwerp"} else "Asia"
    trade.availability_window = availability_window

    # Mock the related order
    order = MagicMock()
    order.product_id = product_id or uuid.uuid4()
    order.delivery_point_id = delivery_point_id or uuid.uuid4()
    order.market_product = market_product
    order.fuel_type = fuel_type
    order.fuel_grade = fuel_grade
    order.region = "Europe" if region in {"Amsterdam", "Rotterdam", "Antwerp"} else "Asia"
    order.delivery_point_name = region
    order.availability_window = availability_window

    # Conflicting mutable relationships prove the tape ignores them.
    order.market_product = "MUTATED_PRODUCT"
    order.fuel_type = "MUTATED_FUEL"
    order.delivery_point_name = "Mutated Port"
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

    def test_fuel_type_from_snapshot(self):
        trade = _make_mock_trade(fuel_type="HSFO")
        entry = _build_tape_entry(trade)
        assert entry.fuel_type == "HSFO"

    def test_market_product_from_snapshot(self):
        trade = _make_mock_trade(market_product="E_METHANOL")
        entry = _build_tape_entry(trade)
        assert entry.market_product == "E_METHANOL"

    def test_exact_scope_includes_product_and_delivery_point_fields(self):
        product_id = uuid.uuid4()
        delivery_point_id = uuid.uuid4()
        trade = _make_mock_trade(
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            region="Singapore",
        )
        entry = _build_tape_entry(trade, expose_delivery_point=True)
        assert entry.product_id == product_id
        assert entry.delivery_point_id == delivery_point_id
        assert entry.delivery_point_name == "Singapore"
        assert entry.scope == "DELIVERY_POINT"

    def test_broad_scope_hides_exact_delivery_point_fields(self):
        trade = _make_mock_trade(region="Singapore")
        entry = _build_tape_entry(trade)
        assert entry.product_id is None
        assert entry.delivery_point_id is None
        assert entry.delivery_point_name is None
        assert entry.scope == "REGION"
        assert entry.region == "Asia"

    def test_exact_scope_uses_delivery_point_name_as_region_label(self):
        trade = _make_mock_trade(region="Singapore")
        entry = _build_tape_entry(trade, expose_delivery_point=True)
        assert entry.region == "Singapore"

    def test_fuel_grade_from_order(self):
        trade = _make_mock_trade(fuel_grade="Green")
        entry = _build_tape_entry(trade)
        assert entry.fuel_grade == "Green"

    def test_availability_window_from_snapshot(self):
        trade = _make_mock_trade(availability_window="Q2 2026")
        entry = _build_tape_entry(trade)
        assert entry.availability_window == "2026-Q2"

    def test_snapshot_only_trade_never_needs_order_relationships(self):
        trade = _make_mock_trade()
        trade.ask_order = None
        trade.bid_order = None
        entry = _build_tape_entry(trade)
        assert entry.fuel_type == "VLSFO"
        assert entry.fuel_grade == "Conventional"
        assert entry.delivery_point_id is None
        assert entry.delivery_point_name is None
        assert entry.region == "Europe"
        assert entry.availability_window == "SPOT"
        assert entry.scope == "REGION"

    def test_mutable_bid_order_is_ignored(self):
        trade = _make_mock_trade(fuel_type="MGO")
        # Move the ask_order to bid_order
        trade.bid_order = trade.ask_order
        trade.ask_order = None
        entry = _build_tape_entry(trade)
        assert entry.fuel_type == "MGO"

    def test_demo_trade_flagged_from_snapshot_provenance(self):
        trade = _make_mock_trade()
        trade.buyer_provenance = "DEMO"
        trade.seller_provenance = "DEMO"
        entry = _build_tape_entry(trade)
        assert entry.is_demo_trade is True
        assert entry.provenance_kind == "DEMO_SEED"

    def test_unknown_trade_is_never_presented_as_real(self):
        trade = _make_mock_trade()
        trade.buyer_provenance = "UNKNOWN"
        trade.seller_provenance = "UNKNOWN"
        entry = _build_tape_entry(trade)
        assert entry.provenance_kind == "UNKNOWN"
        assert entry.demo_status == "UNKNOWN"


# ---------------------------------------------------------------------------
# Schema validation
# ---------------------------------------------------------------------------

class TestTradeTapeSchemas:
    def test_tape_entry_schema_fields(self):
        expected = {
            "id", "product_id", "market_product", "fuel_type", "fuel_grade",
            "delivery_point_id", "delivery_point_name", "region",
            "quantity_mt", "price_per_mt_usd", "total_usd",
            "confirmed_at", "availability_window", "is_demo_trade",
            "scope", "provenance_kind",
            "source_kind", "demo_status",
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
            product_id=uuid.uuid4(),
            market_product="BIO_METHANOL",
            fuel_type="VLSFO",
            fuel_grade="Conventional",
            delivery_point_id=uuid.uuid4(),
            delivery_point_name="Amsterdam",
            region="Amsterdam",
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("750.50"),
            total_usd=Decimal("750500.00"),
            confirmed_at=datetime(2026, 3, 15, 10, 0, 0, tzinfo=timezone.utc),
            availability_window="SPOT",
            scope="DELIVERY_POINT",
            provenance_kind="CONFIRMED_TRADE",
        )
        resp = TradeTapeResponse(items=[entry], total=1, market_hours=True)
        assert resp.total == 1
        assert resp.market_hours is True
        assert len(resp.items) == 1
        assert resp.items[0].id == "abcd1234"


# ---------------------------------------------------------------------------
# Endpoint contract
# ---------------------------------------------------------------------------


class _ScalarResult:
    def __init__(self, value):
        self._value = value

    def scalar(self):
        return self._value


class _TradeResult:
    def __init__(self, trades=None):
        self._trades = trades or []

    def unique(self):
        return self

    def scalars(self):
        return self

    def all(self):
        return self._trades


class TestTradeTapeEndpointContract:
    @pytest.mark.asyncio
    async def test_delivery_point_filter_is_applied_to_query(self):
        delivery_point_id = uuid.uuid4()
        db = MagicMock()
        db.execute = AsyncMock(side_effect=[_ScalarResult(0), _TradeResult()])

        await get_trade_tape(
            db=db,
            fuel_type=None,
            market_product=None,
            delivery_point_id=delivery_point_id,
            region=None,
            availability_window=None,
            skip=0,
            limit=50,
        )

        data_stmt = db.execute.await_args_list[1].args[0]
        compiled_params = data_stmt.compile().params
        assert delivery_point_id in compiled_params.values()

    @pytest.mark.asyncio
    async def test_unfiltered_response_hides_exact_delivery_point_identity(self):
        trade = _make_mock_trade(region="Singapore")
        db = MagicMock()
        db.execute = AsyncMock(side_effect=[_ScalarResult(1), _TradeResult([trade])])

        response = await get_trade_tape(
            db=db,
            fuel_type=None,
            market_product=None,
            delivery_point_id=None,
            region=None,
            availability_window=None,
            skip=0,
            limit=50,
        )

        entry = response.items[0]
        assert entry.scope == "REGION"
        assert entry.region == "Asia"
        assert entry.product_id is None
        assert entry.delivery_point_id is None
        assert entry.delivery_point_name is None

    @pytest.mark.asyncio
    async def test_exact_delivery_point_response_exposes_exact_scope(self):
        delivery_point_id = uuid.uuid4()
        trade = _make_mock_trade(region="Singapore", delivery_point_id=delivery_point_id)
        db = MagicMock()
        db.execute = AsyncMock(side_effect=[_ScalarResult(1), _TradeResult([trade])])

        response = await get_trade_tape(
            db=db,
            fuel_type=None,
            market_product=None,
            delivery_point_id=delivery_point_id,
            region=None,
            availability_window=None,
            skip=0,
            limit=50,
        )

        entry = response.items[0]
        assert entry.scope == "DELIVERY_POINT"
        assert entry.region == "Singapore"
        assert entry.delivery_point_id == delivery_point_id
        assert entry.delivery_point_name == "Singapore"

    def test_openapi_exposes_exact_delivery_point_filter(self):
        from fastapi import FastAPI
        from app.routers.trade_tape import router

        app = FastAPI()
        app.include_router(router, prefix="/api")
        operation = app.openapi()["paths"]["/api/trade-tape"]["get"]
        params = {param["name"]: param for param in operation["parameters"]}

        assert "delivery_point_id" in params
        assert "uuid" in str(params["delivery_point_id"]["schema"])

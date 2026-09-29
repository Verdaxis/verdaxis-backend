"""Clock-bound contracts for generated demo market activity."""

from datetime import UTC, datetime
from decimal import Decimal

from inspect import getsource
from types import SimpleNamespace
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest

from app.market_catalog import B30_SPECIFICATION_STANDARD, B100_SPECIFICATION_STANDARD
from app.models.orderbook import OrderBookStatus, OrderSide
from app.seeds.catalog_seed import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.seeds.market_seed import CI_DATA, PRICING, _seed_price_for_slice, ask_seed_metadata
from app.seeds.forward_monitoring_seed import _default_curve_windows, _demo_slices
from app.services.demo_activity import (
    DEMO_COVERAGE_REFRESH_FIELDS,
    _ask_metadata,
    activity_windows,
    build_demo_market_coverage,
    generate_demo_market_activity,
)
from app.services.execution_policy import order_is_execution_qualified
from app.services import demo_activity


def test_demo_coverage_refresh_preserves_immutable_order_identity():
    assert set(DEMO_COVERAGE_REFRESH_FIELDS).isdisjoint({
        "organization_id",
        "provenance",
        "side",
        "product_id",
        "delivery_point_id",
        "availability_window",
        "inventory_item_id",
    })


def test_activity_windows_roll_forward_without_past_months():
    july = activity_windows(datetime(2026, 7, 20, 12, tzinfo=UTC))
    october = activity_windows(datetime(2026, 10, 1, 0, tzinfo=UTC))

    assert "2026-06" not in july
    assert "2026-Q2" not in july
    assert "2026-07" in july
    assert "2026-Q4" in july

    assert "2026-07" not in october
    assert "2026-10" in october
    assert "2027-Q1" in october
    assert october != july


def test_activity_windows_use_utc_at_a_local_quarter_boundary():
    local = datetime.fromisoformat("2026-10-01T00:30:00+08:00")
    windows = activity_windows(local)
    assert windows == activity_windows(local.astimezone(UTC))
    assert "2026-09" in windows
    assert "2026-10" not in windows


def test_activity_windows_are_deterministic_for_same_clock_tick():
    now = datetime(2026, 7, 20, 12, 3, tzinfo=UTC)

    assert activity_windows(now) == activity_windows(now)


def test_demo_activity_builder_does_not_own_commit_or_rollback():
    source = getsource(generate_demo_market_activity)

    assert ".commit(" not in source
    assert ".rollback(" not in source
    assert "ensure_demo_activity_organizations" not in source


@pytest.mark.parametrize(
    "now,expected_rows",
    [
        (datetime(2026, 9, 30, 12, tzinfo=UTC), 21120),
        (datetime(2026, 10, 1, 12, tzinfo=UTC), 23040),
        (datetime(2026, 12, 31, 12, tzinfo=UTC), 21120),
    ],
)
def test_demo_market_coverage_has_ten_levels_and_contango_across_the_full_horizon(now, expected_rows):
    orders = build_demo_market_coverage(now)
    windows = activity_windows(now)

    assert len(orders) == expected_rows
    assert len({order.idempotency_key for order in orders}) == len(orders)

    slices: dict[tuple[object, object, str], list] = {}
    for order in orders:
        key = (order.product_id, order.delivery_point_id, order.availability_window)
        slices.setdefault(key, []).append(order)

    assert set(slices) == {
        (product, port, window)
        for product in PRODUCT_IDS.values()
        for port in DELIVERY_POINT_IDS.values()
        for window in windows
    }
    top_quotes = {}
    for market_slice, slice_orders in slices.items():
        bids = [order for order in slice_orders if order.side == OrderSide.BID]
        asks = [order for order in slice_orders if order.side == OrderSide.ASK]
        bid_prices = [order.price_per_mt_usd for order in bids]
        ask_prices = [order.price_per_mt_usd for order in asks]
        assert len(bids) == len(set(bid_prices)) == 10
        assert len(asks) == len(set(ask_prices)) == 10
        assert min(bid_prices) > 0
        assert max(bid_prices) < min(ask_prices)
        top_quotes[market_slice] = (max(bid_prices), min(ask_prices))

    for product in PRODUCT_IDS.values():
        for port in DELIVERY_POINT_IDS.values():
            for side_index in (0, 1):
                prices = [top_quotes[product, port, window][side_index] for window in windows]
                assert all(earlier < later for earlier, later in zip(prices, prices[1:])), (product, port, side_index)


@pytest.mark.asyncio
async def test_demo_activity_only_creates_a_matched_pair_at_the_current_window_midpoint(monkeypatch):
    now = datetime(2026, 9, 30, 12, tzinfo=UTC)
    product_name, port_name, window = "B30", "Singapore", "2031-Q3"
    orders = []

    async def assign_order_ids():
        for order in orders:
            if order.id is None:
                order.id = uuid4()

    async def match_pair(db, bid, **kwargs):
        ask = next(order for order in orders if order.side == OrderSide.ASK)
        assert kwargs["allowed_demo_order_pair"] == frozenset((bid.id, ask.id))
        for order in orders:
            order.status = OrderBookStatus.FILLED
            order.remaining_quantity_mt = Decimal("0")
        return [SimpleNamespace(bid_order_id=bid.id, ask_order_id=ask.id)]

    db = SimpleNamespace(add=orders.append, flush=AsyncMock(side_effect=assign_order_ids))
    monkeypatch.setattr(demo_activity, "_tick_trade_exists", AsyncMock(return_value=False))
    monkeypatch.setattr(demo_activity, "_activity_slice", lambda reference: (product_name, port_name, window))
    monkeypatch.setattr(demo_activity, "_load_product_and_port", AsyncMock(return_value=(
        SimpleNamespace(id=PRODUCT_IDS[product_name]), SimpleNamespace(id=DELIVERY_POINT_IDS[port_name]),
    )))
    monkeypatch.setattr(demo_activity, "acquire_market_slice_lock", AsyncMock())
    monkeypatch.setattr(demo_activity, "lock_and_load_market_organizations", AsyncMock())
    monkeypatch.setattr(demo_activity, "match_order", AsyncMock(side_effect=match_pair))

    result = await generate_demo_market_activity(db, now=now)

    bid_lo, bid_hi, ask_lo, ask_hi = PRICING[product_name][port_name]
    midpoint = sum(
        _seed_price_for_slice(
            side, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi,
            window=window, reference_date=now.date(),
        )
        for side in (OrderSide.BID, OrderSide.ASK)
    ) / 2
    assert result["created_orders"] == len(orders) == 2
    assert result["created_trades"] == 1
    assert {order.side for order in orders} == {OrderSide.BID, OrderSide.ASK}
    assert {order.price_per_mt_usd for order in orders} == {midpoint}
    assert all(order.status == OrderBookStatus.FILLED for order in orders)


@pytest.mark.parametrize(
    "product_name,specification,ci_scope,feedstock_detail",
    [
        ("B30", B30_SPECIFICATION_STANDARD, "whole-blend", "VLSFO"),
        ("B100", B100_SPECIFICATION_STANDARD, "whole-fuel", "100% waste-derived FAME"),
    ],
)
def test_biofuel_demo_asks_use_the_contract_and_disclose_synthetic_inputs(
    product_name, specification, ci_scope, feedstock_detail,
):
    now = datetime(2026, 9, 26, 12, tzinfo=UTC)
    fuel_orders = [
        order for order in build_demo_market_coverage(now)
        if order.product_id == PRODUCT_IDS[product_name] and order.side == OrderSide.ASK
    ]

    assert {order.delivery_point_id for order in fuel_orders} == set(DELIVERY_POINT_IDS.values())
    assert {order.availability_window for order in fuel_orders} == set(activity_windows(now))
    for order in fuel_orders:
        assert order.provenance == "DEMO"
        assert order_is_execution_qualified(order)
        assert order.specification_standard == specification
        assert order.certification_declared is True
        assert order.certifications == [order.certification_scheme]
        assert order.msds_available is True
        assert order.is_verdaxis_verified is False
        assert ci_scope in order.carbon_intensity_method
        assert "demo" in order.carbon_intensity_method
        assert feedstock_detail in order.feedstock
        assert CI_DATA[product_name][0] <= order.carbon_intensity_gco2_mj <= CI_DATA[product_name][1]
        assert float(order.energy_density_mj_kg) == CI_DATA[product_name][2]

    for metadata in (
        ask_seed_metadata("ISCC EU", product_name=product_name),
        _ask_metadata(product_name, "Singapore", "SPOT"),
    ):
        assert metadata["specification_standard"] == specification
        assert ci_scope in metadata["carbon_intensity_method"]
        assert "not certified" in metadata["carbon_intensity_method"]


@pytest.mark.parametrize("product_name", ["B30", "B100"])
def test_biofuel_demo_forward_monitoring_covers_spot_and_singapore_curve(product_name):
    now = datetime(2026, 9, 26, 12, tzinfo=UTC)
    fuel_slices = {
        (port, window) for product, port, window in _demo_slices(now)
        if product == product_name
    }

    assert {(port, "SPOT") for port in DELIVERY_POINT_IDS} <= fuel_slices
    assert {("Singapore", window) for window in _default_curve_windows(now)} <= fuel_slices

"""Clock-bound contracts for generated demo market activity."""

from datetime import UTC, datetime

from inspect import getsource

import pytest

from app.market_catalog import B30_SPECIFICATION_STANDARD, B100_SPECIFICATION_STANDARD
from app.models.orderbook import OrderSide
from app.seeds.catalog_seed import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.seeds.market_seed import CI_DATA, ask_seed_metadata
from app.seeds.forward_monitoring_seed import _default_curve_windows, _demo_slices
from app.services.demo_activity import (
    DEMO_COVERAGE_REFRESH_FIELDS,
    _ask_metadata,
    activity_windows,
    build_demo_market_coverage,
    generate_demo_market_activity,
)
from app.services.execution_policy import order_is_execution_qualified


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


def test_activity_windows_are_deterministic_for_same_clock_tick():
    now = datetime(2026, 7, 20, 12, 3, tzinfo=UTC)

    assert activity_windows(now) == activity_windows(now)


def test_demo_activity_builder_does_not_own_commit_or_rollback():
    source = getsource(generate_demo_market_activity)

    assert ".commit(" not in source
    assert ".rollback(" not in source
    assert "ensure_demo_activity_organizations" not in source


def test_demo_market_coverage_restores_full_non_crossed_book():
    orders = build_demo_market_coverage(datetime(2026, 7, 28, 12, tzinfo=UTC))

    assert len(orders) == 2688
    assert (
        len(build_demo_market_coverage(datetime(2026, 8, 15, 12, tzinfo=UTC)))
        == 2688
    )
    assert (
        len(build_demo_market_coverage(datetime(2026, 9, 15, 12, tzinfo=UTC)))
        == 2688
    )
    assert len({order.idempotency_key for order in orders}) == len(orders)

    slices: dict[tuple[object, object, str], list] = {}
    for order in orders:
        key = (order.product_id, order.delivery_point_id, order.availability_window)
        slices.setdefault(key, []).append(order)

    assert len(slices) == 6 * 8 * 24
    for (_product, _port, window), slice_orders in slices.items():
        bids = [order for order in slice_orders if order.side == OrderSide.BID]
        asks = [order for order in slice_orders if order.side == OrderSide.ASK]
        expected_depth = 2 if window == "SPOT" or window.startswith("2026-0") else 1
        assert len(bids) == expected_depth
        assert len(asks) == expected_depth
        assert max(order.price_per_mt_usd for order in bids) < min(
            order.price_per_mt_usd for order in asks
        )


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

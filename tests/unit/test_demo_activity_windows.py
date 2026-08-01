"""Clock-bound contracts for generated demo market activity."""

from datetime import UTC, datetime

from inspect import getsource

from app.models.orderbook import OrderSide
from app.services.demo_activity import (
    DEMO_COVERAGE_REFRESH_FIELDS,
    activity_windows,
    build_demo_market_coverage,
    generate_demo_market_activity,
)


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

    assert len(orders) == 1024
    assert (
        len(build_demo_market_coverage(datetime(2026, 8, 15, 12, tzinfo=UTC)))
        == 1024
    )
    assert (
        len(build_demo_market_coverage(datetime(2026, 9, 15, 12, tzinfo=UTC)))
        == 1024
    )
    assert len({order.idempotency_key for order in orders}) == len(orders)

    slices: dict[tuple[object, object, str], list] = {}
    for order in orders:
        key = (order.product_id, order.delivery_point_id, order.availability_window)
        slices.setdefault(key, []).append(order)

    assert len(slices) == 4 * 8 * 12
    for (_product, _port, window), slice_orders in slices.items():
        bids = [order for order in slice_orders if order.side == OrderSide.BID]
        asks = [order for order in slice_orders if order.side == OrderSide.ASK]
        expected_depth = 2 if window == "SPOT" or window.startswith("2026-0") else 1
        assert len(bids) == expected_depth
        assert len(asks) == expected_depth
        assert max(order.price_per_mt_usd for order in bids) < min(
            order.price_per_mt_usd for order in asks
        )

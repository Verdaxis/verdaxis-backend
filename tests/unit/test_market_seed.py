"""Unit tests for executable-market seed rules."""

from datetime import date, datetime, timedelta, timezone
from decimal import Decimal
from inspect import getsource

import pytest

from types import SimpleNamespace

from app.seeds.market_seed import (
    BUYER_ORGS,
    PRICING,
    SUPPLIER_ORGS,
    WINDOWS,
    _clamp_trade_timeline,
    _orders_share_executable_slice,
    _seed_order,
    _seed_trade,
    _seed_price_for_slice,
    _slice_certification_scheme,
    ask_seed_metadata,
    bid_seed_metadata,
    build_seed_windows,
    validate_demo_reset,
)
from app.models.orderbook import Initiator, OrderSide


def test_market_seed_windows_cover_current_and_forward_slices():
    today = datetime.now(timezone.utc).date()
    expected = build_seed_windows(today)
    current_month = f"{today.year}-{today.month:02d}"
    current_quarter = ((today.month - 1) // 3) + 1
    current_quarter_end_month = current_quarter * 3
    current_quarter_months = [
        f"{today.year}-{month:02d}"
        for month in range(today.month, current_quarter_end_month + 1)
    ]

    assert WINDOWS == expected
    assert 'SPOT' in WINDOWS
    assert current_month in WINDOWS
    assert all(month in WINDOWS for month in current_quarter_months)
    assert any(window.endswith(f"-Q{(current_quarter % 4) + 1}") for window in WINDOWS)


def test_demo_seed_organizations_are_clearly_fictitious_and_deterministic():
    assert [org["name"] for org in BUYER_ORGS] == [
        f"Verdaxis Demo Buyer {index:02d}" for index in range(1, 6)
    ]
    assert [org["name"] for org in SUPPLIER_ORGS] == [
        f"Verdaxis Demo Supplier {index:02d}" for index in range(1, 6)
    ]
    assert len({org["id"] for org in (*BUYER_ORGS, *SUPPLIER_ORGS)}) == 10


def test_demo_seed_order_always_has_an_explicit_expiry():
    observed_at = datetime(2026, 7, 20, 8, tzinfo=timezone.utc)
    order = _seed_order(
        availability_window="2026-07",
        created_at=observed_at,
    )

    assert order.provenance == "DEMO"
    assert order.expires_at is not None
    assert order.expires_at > observed_at


def test_seed_trade_derives_initiating_tenant_from_side():
    buyer_id = BUYER_ORGS[0]["id"]
    seller_id = SUPPLIER_ORGS[0]["id"]

    buyer_trade = _seed_trade(
        buyer_id=buyer_id,
        seller_id=seller_id,
        initiated_by=Initiator.BUYER,
    )
    seller_trade = _seed_trade(
        buyer_id=buyer_id,
        seller_id=seller_id,
        initiated_by=Initiator.SELLER,
    )

    assert buyer_trade.initiator_org_id == buyer_id
    assert seller_trade.initiator_org_id == seller_id


def test_market_seed_never_generates_executable_rfq_acceptance():
    source = getsource(__import__("app.seeds.market_seed", fromlist=["seed_market_data"]).seed_market_data)

    assert "RFQStatus.ACCEPTED" not in source


def test_market_seed_ask_metadata_declares_certification():
    metadata = ask_seed_metadata()
    assert metadata['certification_declared'] is True
    assert metadata['certification_scheme']
    assert metadata['certifications'] == [metadata['certification_scheme']]
    assert metadata['specification_standard']
    assert metadata['msds_available'] is True
    assert metadata['feedstock']
    assert metadata['carbon_intensity_method']


def test_market_seed_bid_metadata_declares_execution_scheme():
    metadata = bid_seed_metadata()
    assert metadata['certification_scheme']


def test_trade_pairing_requires_exact_executable_slice():
    bid = SimpleNamespace(
        product_id='product-1',
        delivery_point_id='port-1',
        availability_window='SPOT',
        certification_scheme='ISCC EU',
    )
    matching_ask = SimpleNamespace(
        product_id='product-1',
        delivery_point_id='port-1',
        availability_window='SPOT',
        certification_scheme='ISCC EU',
    )
    different_window_ask = SimpleNamespace(
        product_id='product-1',
        delivery_point_id='port-1',
        availability_window='2026-Q3',
        certification_scheme='ISCC EU',
    )
    different_cert_ask = SimpleNamespace(
        product_id='product-1',
        delivery_point_id='port-1',
        availability_window='SPOT',
        certification_scheme='ISCC PLUS',
    )

    assert _orders_share_executable_slice(bid, matching_ask) is True
    assert _orders_share_executable_slice(bid, different_window_ask) is False
    assert _orders_share_executable_slice(bid, different_cert_ask) is False


def test_slice_certification_scheme_is_deterministic_and_valid():
    scheme_a = _slice_certification_scheme('Bio Methanol', 'Singapore', 'SPOT')
    scheme_b = _slice_certification_scheme('Bio Methanol', 'Singapore', 'SPOT')

    assert scheme_a == scheme_b
    assert scheme_a in {'ISCC EU', 'ISCC PLUS', 'REDcert EU'}


def test_seed_price_for_slice_keeps_resting_book_non_crossed():
    best_bid = _seed_price_for_slice(
        OrderSide.BID,
        bid_lo=1020,
        bid_hi=1070,
        ask_lo=1090,
        ask_hi=1140,
        window='SPOT',
        depth_index=0,
    )
    best_ask = _seed_price_for_slice(
        OrderSide.ASK,
        bid_lo=1020,
        bid_hi=1070,
        ask_lo=1090,
        ask_hi=1140,
        window='SPOT',
        depth_index=0,
    )

    assert best_ask > best_bid


@pytest.mark.parametrize(
    "reference_date,window,expected_premium",
    [
        (date(2026, 9, 1), "2026-09", "1.23"),  # 15 days to delivery midpoint.
        (date(2026, 9, 30), "2026-09", "0.04"),  # Half of the remaining day.
        (date(2026, 9, 30), "2026-Q4", "3.86"),  # 47 days.
        (date(2026, 12, 31), "2027-Q1", "3.78"),  # 46 days.
        (date(2028, 2, 29), "2028-Q2", "6.37"),  # 77.5 days; ACT/365 in leap years.
        (date(2027, 1, 1), "2028-CAL", "45.04"),  # 548 days; no compounding.
    ],
)
def test_seed_prices_apply_three_percent_carry_to_remaining_delivery_midpoint(
    reference_date, window, expected_premium,
):
    # A $1,000 Spot midpoint earns $30 per 365 days on both sides.
    for side, spot_quote in ((OrderSide.BID, "990"), (OrderSide.ASK, "1010")):
        price = _seed_price_for_slice(
            side,
            bid_lo=990,
            bid_hi=990,
            ask_lo=1010,
            ask_hi=1010,
            window=window,
            reference_date=reference_date,
        )
        assert price == Decimal(spot_quote) + Decimal(expected_premium)


def test_seed_prices_preserve_every_spot_market_anchor():
    for product_name, ports in PRICING.items():
        for port_name, (bid_lo, bid_hi, ask_lo, ask_hi) in ports.items():
            for side, low, high in (
                (OrderSide.BID, bid_lo, bid_hi),
                (OrderSide.ASK, ask_lo, ask_hi),
            ):
                price = _seed_price_for_slice(
                    side,
                    bid_lo=bid_lo,
                    bid_hi=bid_hi,
                    ask_lo=ask_lo,
                    ask_hi=ask_hi,
                    window="SPOT",
                    reference_date=date(2032, 1, 1),
                )
                assert price == Decimal(str((low + high) / 2)), (product_name, port_name, side)


def test_clamp_trade_timeline_caps_seeded_trade_dates_at_now():
    now = datetime(2026, 4, 14, 12, 0, tzinfo=timezone.utc)
    created = datetime(2026, 4, 14, 11, 30, tzinfo=timezone.utc)
    confirmed = now + timedelta(hours=3)
    delivered = now + timedelta(days=2)
    paid = now + timedelta(days=4)

    _, confirmed, delivered, paid = _clamp_trade_timeline(created, confirmed, delivered, paid, now)

    assert confirmed == now
    assert delivered == now
    assert paid == now


def test_demo_reset_attests_staging_url_and_connected_database():
    validate_demo_reset(
        environment="staging",
        explicit_opt_in=True,
        database_url="postgresql+asyncpg://seed:secret@db/verdaxis_staging",
        current_database="verdaxis_staging",
    )


def test_demo_reset_rejects_production_database_under_staging_label_and_inverse():
    for environment, database_name in (
        ("staging", "verdaxis"),
        ("production", "verdaxis_staging"),
    ):
        with pytest.raises(RuntimeError):
            validate_demo_reset(
                environment=environment,
                explicit_opt_in=True,
                database_url=(
                    f"postgresql+asyncpg://seed:secret@db/{database_name}"
                ),
                current_database=database_name,
            )

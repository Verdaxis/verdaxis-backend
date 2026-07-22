"""Unit tests for executable-market seed rules."""

from datetime import date, datetime, timedelta, timezone
from inspect import getsource

import pytest

from types import SimpleNamespace

from app.seeds.market_seed import (
    BUYER_ORGS,
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
    today = date.today()
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


def test_seed_price_for_slice_builds_plausible_contango():
    month_window = next(window for window in WINDOWS if len(window) == 7 and window[4] == '-')
    quarter_window = next(window for window in WINDOWS if '-Q' in window)

    spot_mid = (
        _seed_price_for_slice(
            OrderSide.BID,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window='SPOT',
            depth_index=0,
        )
        + _seed_price_for_slice(
            OrderSide.ASK,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window='SPOT',
            depth_index=0,
        )
    ) / 2
    month_mid = (
        _seed_price_for_slice(
            OrderSide.BID,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window=month_window,
            depth_index=0,
        )
        + _seed_price_for_slice(
            OrderSide.ASK,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window=month_window,
            depth_index=0,
        )
    ) / 2
    quarter_mid = (
        _seed_price_for_slice(
            OrderSide.BID,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window=quarter_window,
            depth_index=0,
        )
        + _seed_price_for_slice(
            OrderSide.ASK,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window=quarter_window,
            depth_index=0,
        )
    ) / 2

    assert month_mid > spot_mid
    assert quarter_mid > month_mid


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

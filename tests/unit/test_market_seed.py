"""Unit tests for executable-market seed rules."""

from datetime import date, datetime, timedelta, timezone

from types import SimpleNamespace

from app.seeds.market_seed import WINDOWS, ask_seed_metadata, bid_seed_metadata, build_seed_windows, _orders_share_executable_slice, _seed_price_for_slice, _slice_certification_scheme, _clamp_trade_timeline
from app.models.orderbook import OrderSide


def test_market_seed_windows_cover_current_and_forward_slices():
    expected = build_seed_windows(date(2026, 4, 13))
    assert WINDOWS == expected
    assert 'SPOT' in WINDOWS
    assert '2026-04' in WINDOWS
    assert '2026-06' in WINDOWS
    assert '2026-Q3' in WINDOWS


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
            window='2026-05',
            depth_index=0,
        )
        + _seed_price_for_slice(
            OrderSide.ASK,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window='2026-05',
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
            window='2026-Q3',
            depth_index=0,
        )
        + _seed_price_for_slice(
            OrderSide.ASK,
            bid_lo=1020,
            bid_hi=1070,
            ask_lo=1090,
            ask_hi=1140,
            window='2026-Q3',
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

"""Unit tests for executable-market seed rules."""

from datetime import date

from types import SimpleNamespace

from app.seeds.market_seed import WINDOWS, ask_seed_metadata, bid_seed_metadata, build_seed_windows, _orders_share_executable_slice


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

"""Unit tests for executable-market seed rules."""

from datetime import date

from app.seeds.market_seed import PRICING, WINDOWS, ask_seed_metadata, build_seed_windows


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

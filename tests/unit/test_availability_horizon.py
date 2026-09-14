"""Delivery horizons roll forward in both selection and admission."""

from datetime import date

from app.services.availability_windows import (
    is_tradable_availability_window,
    tradable_availability_windows,
)


def test_five_year_horizon_rolls_across_year_boundary():
    for today, final, outside in (
        (date(2026, 9, 11), "2031-Q3", "2031-Q4"),
        (date(2026, 12, 31), "2031-Q4", "2032-Q1"),
        (date(2027, 1, 1), "2032-Q1", "2032-Q2"),
    ):
        windows = tradable_availability_windows(today=today)
        assert windows[0] == "SPOT"
        assert windows[-1] == final
        assert len([window for window in windows if "-Q" in window]) == 20
        assert is_tradable_availability_window(final, today=today)
        assert not is_tradable_availability_window(outside, today=today)

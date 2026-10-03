"""Delivery horizons roll forward in both selection and admission."""

from datetime import date

from app.services.availability_windows import (
    forward_monitoring_default_windows,
    is_tradable_availability_window,
    tradable_availability_windows,
)


def test_delivery_horizons_roll_across_year_boundary():
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

    assert forward_monitoring_default_windows(
        reference_date=date(2026, 12, 31),
    ) == [
        "SPOT",
        "2027-01",
        "2027-02",
        "2027-03",
        "2027-04",
        "2027-05",
        "2027-06",
        "2027-Q1",
        "2027-Q2",
        "2027-Q3",
        "2027-Q4",
        "2027-CAL",
        "2028-CAL",
    ]

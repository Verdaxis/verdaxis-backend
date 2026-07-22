"""Clock-bound contracts for generated demo market activity."""

from datetime import UTC, datetime

from inspect import getsource

from app.services.demo_activity import activity_windows, generate_demo_market_activity


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

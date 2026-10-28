"""The clock manifest must describe the source it patches.

`clock.py` patches named symbols to one explicit instant. These tests prove the
manifest still matches the imported source, so a moved or renamed symbol fails
loudly instead of leaving a clock unfrozen.
"""

from datetime import UTC, date, datetime

import pytest

from tests.rust_contract import clock


def test_manifest_matches_source():
    assert clock.manifest_gaps() == []


def test_clock_requires_the_test_environment(monkeypatch):
    monkeypatch.delenv("ENVIRONMENT", raising=False)
    with pytest.raises(RuntimeError):
        clock.require_test_environment()


def test_naive_instant_is_refused():
    with pytest.raises(ValueError):
        clock.fixed_clock_patches(datetime(2026, 10, 15, 12, 0))


def test_frozen_datetime_replaces_the_module_clock():
    instant = datetime(2026, 10, 15, 12, 0, tzinfo=UTC)
    eligibility = clock.module("app.services.market_data_eligibility")
    with clock.fixed_clock_patches(instant):
        assert eligibility.datetime.now(UTC) == instant
        assert eligibility.datetime.now() == instant.replace(tzinfo=None)
    assert eligibility.datetime is datetime


def test_frozen_date_replaces_date_today():
    instant = datetime(2026, 10, 15, 12, 0, tzinfo=UTC)
    windows = clock.module("app.services.availability_windows")
    with clock.fixed_clock_patches(instant):
        assert windows.date.today() == date(2026, 10, 15)

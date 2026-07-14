"""Validation tests for the Product Analytics query and response schemas."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta, timezone
from decimal import Decimal
from uuid import uuid4

import pytest
from pydantic import ValidationError

from app.schemas.product_analytics import (
    AggregateCell,
    AnalyticsActivity,
    AnalyticsAudience,
    AnalyticsCoverage,
    AnalyticsDataQuality,
    AnalyticsMeta,
    AnalyticsSourceCoverage,
    AnalyticsSourceStatus,
    MAX_RANGE_DAYS,
    ProductAnalyticsQuery,
    ReferenceCoverageRow,
    ReferenceScope,
    ReferenceSourceKind,
    ReferenceSourceLabel,
    SeriesPoint,
    ActivityTrend,
)

_START = datetime(2026, 6, 1, tzinfo=UTC)
_END = datetime(2026, 7, 1, tzinfo=UTC)


def _query(**overrides) -> ProductAnalyticsQuery:
    values = {"start": _START, "end": _END}
    values.update(overrides)
    return ProductAnalyticsQuery(**values)


# ---------------------------------------------------------------------------
# UTC normalization
# ---------------------------------------------------------------------------


def test_naive_datetimes_are_treated_as_utc():
    query = _query(start=datetime(2026, 6, 1), end=datetime(2026, 7, 1))
    assert query.start == _START
    assert query.end == _END
    assert query.start.tzinfo == UTC


def test_aware_datetimes_are_converted_to_utc():
    singapore = timezone(timedelta(hours=8))
    query = _query(
        start=datetime(2026, 6, 1, 8, tzinfo=singapore),
        end=datetime(2026, 7, 1, 8, tzinfo=singapore),
    )
    assert query.start == _START
    assert query.end == _END
    assert query.start.utcoffset() == timedelta(0)


# ---------------------------------------------------------------------------
# Range validation
# ---------------------------------------------------------------------------


def test_end_must_be_strictly_after_start():
    with pytest.raises(ValidationError):
        _query(end=_START)
    with pytest.raises(ValidationError):
        _query(start=_END, end=_START)


def test_range_is_bounded_to_365_days():
    exact = _query(end=_START + timedelta(days=MAX_RANGE_DAYS))
    assert exact.end - exact.start == timedelta(days=365)
    with pytest.raises(ValidationError):
        _query(end=_START + timedelta(days=MAX_RANGE_DAYS, seconds=1))


# ---------------------------------------------------------------------------
# Previous period
# ---------------------------------------------------------------------------


def test_previous_period_is_the_immediately_preceding_equivalent_interval():
    query = _query()
    assert query.previous_end == _START
    assert query.previous_start == _START - (_END - _START)
    # Half-open mirror: same length, ends exactly at start.
    assert query.previous_end - query.previous_start == query.end - query.start


def test_previous_period_is_absent_when_compare_is_off():
    query = _query(compare=False)
    assert query.previous_start is None
    assert query.previous_end is None


# ---------------------------------------------------------------------------
# Enums and marketplace filters
# ---------------------------------------------------------------------------


def test_audience_and_activity_parse_from_strings():
    query = _query(audience="BUYER", activity="REFERENCE")
    assert query.audience == AnalyticsAudience.BUYER
    assert query.activity == AnalyticsActivity.REFERENCE


def test_defaults_are_all_audience_and_live_activity():
    query = _query()
    assert query.audience == AnalyticsAudience.ALL
    assert query.activity == AnalyticsActivity.LIVE
    assert query.compare is True


@pytest.mark.parametrize("field,value", [("audience", "EVERYONE"), ("activity", "MIXED")])
def test_unknown_enum_values_are_rejected(field, value):
    with pytest.raises(ValidationError):
        _query(**{field: value})


def test_marketplace_filters_are_optional_and_typed():
    empty = _query()
    assert empty.product_id is None
    assert empty.delivery_point_id is None
    assert empty.availability_window is None

    product_id = uuid4()
    query = _query(
        product_id=str(product_id),
        delivery_point_id=product_id,
        availability_window="2026-Q3",
    )
    assert query.product_id == product_id
    assert query.availability_window == "2026-Q3"

    with pytest.raises(ValidationError):
        _query(product_id="not-a-uuid")
    with pytest.raises(ValidationError):
        _query(availability_window="X" * 17)


# ---------------------------------------------------------------------------
# Suppression and meta primitives
# ---------------------------------------------------------------------------


def test_suppressed_cells_never_carry_counts():
    suppressed = AggregateCell(key="BUYER", suppressed=True)
    assert suppressed.count is None
    with pytest.raises(ValidationError):
        AggregateCell(key="BUYER", count=2, suppressed=True)
    genuine_zero = AggregateCell(key="SUPPLIER", count=0)
    assert genuine_zero.suppressed is False


def test_meta_carries_per_source_coverage_and_data_quality():
    coverage = AnalyticsCoverage(
        authoritative=AnalyticsSourceCoverage(status=AnalyticsSourceStatus.AVAILABLE),
        behavioral=AnalyticsSourceCoverage(status=AnalyticsSourceStatus.UNAVAILABLE),
        login_history=AnalyticsSourceCoverage(status=AnalyticsSourceStatus.PARTIAL),
        status_history=AnalyticsSourceCoverage(status=AnalyticsSourceStatus.AVAILABLE),
        reference=AnalyticsSourceCoverage.not_applicable(),
    )
    meta = AnalyticsMeta(
        start=_START,
        end=_END,
        previous_start=_START - timedelta(days=30),
        previous_end=_START,
        observed_at=_END,
        coverage=coverage,
        data_quality=AnalyticsDataQuality(),
    )
    assert meta.coverage.reference.status == AnalyticsSourceStatus.NOT_APPLICABLE
    assert meta.data_quality.cohort_complete is True
    assert meta.data_quality.suppressed_cell_count == 0


def test_series_are_bounded_to_366_points():
    points = [
        SeriesPoint(date=(_START + timedelta(days=i)).date(), value=i) for i in range(366)
    ]
    trend = ActivityTrend(visitors=points)
    assert len(trend.visitors) == 366
    overflow = points + [SeriesPoint(date=_END.date(), value=0)]
    with pytest.raises(ValidationError):
        ActivityTrend(visitors=overflow)


# ---------------------------------------------------------------------------
# Reference coverage invariants (§1.6)
# ---------------------------------------------------------------------------


def _reference_row(**overrides) -> ReferenceCoverageRow:
    values = {
        "product_key": "BIO_METHANOL",
        "product_label": "Bio Methanol",
        "delivery_point_key": "singapore",
        "delivery_point_label": "Singapore",
        "availability_window": "SPOT",
        "availability_window_label": "Spot",
        "benchmark_price_usd_per_mt": Decimal("812.50"),
        "source_label": ReferenceSourceLabel.MANUAL_OVERRIDE,
        "generated_at": _END,
        "observed_at": _END - timedelta(hours=4),
        "source_kind": ReferenceSourceKind.ADMIN_BENCHMARK,
        "scope": ReferenceScope.EXACT_SLICE,
        "coverage_status": "current",
    }
    values.update(overrides)
    return ReferenceCoverageRow(**values)


def test_current_reference_coverage_requires_price_and_observation():
    assert _reference_row().coverage_status == "current"
    with pytest.raises(ValidationError):
        _reference_row(observed_at=None)
    with pytest.raises(ValidationError):
        _reference_row(benchmark_price_usd_per_mt=None)


def test_stale_reference_coverage_may_keep_price_without_observation():
    stale = _reference_row(
        coverage_status="stale",
        observed_at=None,
        source_label=ReferenceSourceLabel.SEED_MATRIX,
        source_kind=ReferenceSourceKind.SEEDED_BENCHMARK,
        scope=ReferenceScope.WINDOW_ADJUSTED,
    )
    assert stale.benchmark_price_usd_per_mt is not None
    assert stale.observed_at is None


def test_unavailable_reference_coverage_exposes_neither_price_nor_observation():
    unavailable = _reference_row(
        coverage_status="unavailable",
        benchmark_price_usd_per_mt=None,
        observed_at=None,
    )
    assert unavailable.benchmark_price_usd_per_mt is None
    with pytest.raises(ValidationError):
        _reference_row(coverage_status="unavailable", observed_at=None)
    with pytest.raises(ValidationError):
        _reference_row(coverage_status="unavailable", benchmark_price_usd_per_mt=None)

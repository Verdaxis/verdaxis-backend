"""Tests for shared market activity provenance helpers."""
from datetime import UTC, datetime

from app.schemas.market_activity import (
    MarketDataProvenance,
    MarketDemoStatus,
    MarketScope,
    MarketSourceKind,
    demo_status_from_counts,
    source_kind_from_counts,
)


def test_demo_status_precedence_marks_empty_as_not_applicable():
    assert demo_status_from_counts() == MarketDemoStatus.NOT_APPLICABLE


def test_demo_status_precedence_marks_unknown_as_unknown():
    assert demo_status_from_counts(real_count=2, demo_count=1, unknown_count=1) == MarketDemoStatus.UNKNOWN


def test_demo_status_precedence_marks_real_demo_mix():
    assert demo_status_from_counts(real_count=2, demo_count=1) == MarketDemoStatus.MIXED


def test_source_kind_precedence_uses_unknown_before_mixed():
    assert source_kind_from_counts(
        real_count=2,
        demo_count=1,
        unknown_count=1,
        real_source=MarketSourceKind.CONFIRMED_TRADE,
    ) == MarketSourceKind.UNKNOWN


def test_source_kind_precedence_maps_demo_only_to_demo_seed():
    assert source_kind_from_counts(
        demo_count=1,
        real_source=MarketSourceKind.LIVE_ORDER,
    ) == MarketSourceKind.DEMO_SEED


def test_market_data_provenance_serializes_enum_values():
    observed_at = datetime(2026, 6, 17, tzinfo=UTC)
    payload = MarketDataProvenance(
        source_kind=MarketSourceKind.CONFIRMED_TRADE,
        scope=MarketScope.DELIVERY_POINT,
        demo_status=MarketDemoStatus.REAL_ONLY,
        observed_at=observed_at,
        real_count=3,
    ).model_dump(mode="json")

    assert payload["source_kind"] == "CONFIRMED_TRADE"
    assert payload["scope"] == "DELIVERY_POINT"
    assert payload["demo_status"] == "REAL_ONLY"
    assert payload["observed_at"] == "2026-06-17T00:00:00Z"

"""Shared market activity provenance schemas."""
from __future__ import annotations

from datetime import datetime
from enum import Enum

from pydantic import BaseModel


class MarketSourceKind(str, Enum):
    CONFIRMED_TRADE = "CONFIRMED_TRADE"
    LIVE_ORDER = "LIVE_ORDER"
    DEMO_SEED = "DEMO_SEED"
    BENCHMARK_REFERENCE = "BENCHMARK_REFERENCE"
    MIXED_SOURCE = "MIXED_SOURCE"
    NO_DATA = "NO_DATA"
    UNKNOWN = "UNKNOWN"


class MarketScope(str, Enum):
    DELIVERY_POINT = "DELIVERY_POINT"
    REGION = "REGION"
    PRODUCT = "PRODUCT"
    UNKNOWN = "UNKNOWN"


class MarketDemoStatus(str, Enum):
    REAL_ONLY = "REAL_ONLY"
    DEMO_ONLY = "DEMO_ONLY"
    MIXED = "MIXED"
    UNKNOWN = "UNKNOWN"
    NOT_APPLICABLE = "NOT_APPLICABLE"


class MarketDataProvenance(BaseModel):
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    scope: MarketScope = MarketScope.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN
    is_reference: bool = False
    observed_at: datetime | None = None
    real_count: int = 0
    demo_count: int = 0
    unknown_count: int = 0
    detail: str | None = None


def demo_status_from_counts(*, real_count: int = 0, demo_count: int = 0, unknown_count: int = 0) -> MarketDemoStatus:
    """Return aggregate demo status using the v1 precedence rules."""
    if real_count <= 0 and demo_count <= 0 and unknown_count <= 0:
        return MarketDemoStatus.NOT_APPLICABLE
    if unknown_count > 0:
        return MarketDemoStatus.UNKNOWN
    if real_count > 0 and demo_count > 0:
        return MarketDemoStatus.MIXED
    if demo_count > 0:
        return MarketDemoStatus.DEMO_ONLY
    return MarketDemoStatus.REAL_ONLY


def source_kind_from_counts(
    *,
    real_count: int = 0,
    demo_count: int = 0,
    unknown_count: int = 0,
    real_source: MarketSourceKind,
) -> MarketSourceKind:
    """Return aggregate source kind using the v1 demo-status precedence rules."""
    status = demo_status_from_counts(real_count=real_count, demo_count=demo_count, unknown_count=unknown_count)
    if status == MarketDemoStatus.NOT_APPLICABLE:
        return MarketSourceKind.NO_DATA
    if status == MarketDemoStatus.UNKNOWN:
        return MarketSourceKind.UNKNOWN
    if status == MarketDemoStatus.MIXED:
        return MarketSourceKind.MIXED_SOURCE
    if status == MarketDemoStatus.DEMO_ONLY:
        return MarketSourceKind.DEMO_SEED
    return real_source

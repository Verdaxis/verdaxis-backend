from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field

from app.services.behavioral_analytics import AnalyticsDiagnostic


class MetricEntry(BaseModel):
    name: str
    value: int = Field(ge=0)


class DailyVisitorPoint(BaseModel):
    date: str
    value: int = Field(ge=0)


class DailyEventPoint(BaseModel):
    date: str
    event: str
    value: int = Field(ge=0)


class BehavioralUsage(BaseModel):
    visitors: int = Field(ge=0)
    visits: int = Field(ge=0)
    pageviews: int = Field(ge=0)
    total_time_seconds: int = Field(ge=0)
    average_session_duration_seconds: float = Field(ge=0)
    event_totals: dict[str, int]
    event_series: list[DailyEventPoint]
    daily_visitors: list[DailyVisitorPoint]
    top_entries: list[MetricEntry]
    top_referrers: list[MetricEntry]


class AuthoritativeUsage(BaseModel):
    registrations: int = Field(ge=0)
    users_logging_in: int = Field(ge=0)
    order_placing_organizations: int = Field(ge=0)


class FunnelStage(BaseModel):
    name: str
    value: int = Field(ge=0)
    conversion_from_previous_pct: float | None = Field(default=None, ge=0)


class ProductUsageResponse(BaseModel):
    days: Literal[7, 30, 90]
    period_start: datetime
    period_end: datetime
    behavioral_status: Literal["available", "unavailable"]
    diagnostic: AnalyticsDiagnostic | None
    observed_at: datetime
    behavioral: BehavioralUsage
    authoritative: AuthoritativeUsage
    funnel: list[FunnelStage]

"""Strict request and response contracts for per-user activity."""

from __future__ import annotations

from datetime import datetime
from typing import Any, Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from app.market_catalog import CANONICAL_DELIVERY_POINTS, MarketProduct
from app.services.availability_windows import JSON_SCHEMA_PATTERN


class BrowsingEventIn(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: UUID
    action: Literal["page_view", "market_view", "market_filter"]
    page: Literal[
        "home",
        "map",
        "marketplace",
        "curve",
        "watchlist",
        "analytics",
        "trades",
        "quotes",
        "compliance",
        "training",
        "settings",
        "admin",
    ]
    market_product: MarketProduct | None = None
    delivery_point_id: UUID | None = None
    availability_window: str | None = Field(default=None, pattern=JSON_SCHEMA_PATTERN)

    @field_validator("delivery_point_id")
    @classmethod
    def delivery_point_must_be_canonical(cls, value: UUID | None) -> UUID | None:
        if value is not None and value not in {point.id for point in CANONICAL_DELIVERY_POINTS}:
            raise ValueError("delivery_point_id must identify a canonical delivery point")
        return value


class ActivityDeliveryLossIn(BaseModel):
    model_config = ConfigDict(extra="forbid")

    report_id: UUID
    dropped_events: int = Field(ge=0, le=10_000)
    rejected_events: int = Field(ge=0, le=10_000)

    @model_validator(mode="after")
    def require_reported_loss(self) -> "ActivityDeliveryLossIn":
        if self.dropped_events + self.rejected_events == 0:
            raise ValueError("delivery_loss must report at least one lost event")
        return self


class BrowsingEventsIn(BaseModel):
    model_config = ConfigDict(extra="forbid")

    # Deprecated compatibility marker. New clients use the separate privacy
    # policy and omit this field.
    consent_version: Literal[2] | None = None
    events: list[BrowsingEventIn] = Field(min_length=1, max_length=50)
    delivery_loss: ActivityDeliveryLossIn | None = None


class BrowsingEventsAccepted(BaseModel):
    accepted: int


class UserActivityItem(BaseModel):
    id: str
    occurred_at: datetime
    source: Literal["browsing", "business", "login"]
    action: str
    details: dict[str, Any]


class BrowserReportedDeliveryLoss(BaseModel):
    """Partial lower bound; silent or offline clients remain unknown."""

    coverage: Literal["partial"] = "partial"
    reports_received: int
    dropped_events: int
    rejected_events: int
    last_reported_at: datetime | None


class UserActivityPage(BaseModel):
    items: list[UserActivityItem]
    has_more: bool
    last_activity_at: datetime | None
    browser_reported_delivery_loss: BrowserReportedDeliveryLoss

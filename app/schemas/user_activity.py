"""Strict request and response contracts for per-user activity."""

from __future__ import annotations

from datetime import datetime
from typing import Any, Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator

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


class BrowsingEventsIn(BaseModel):
    model_config = ConfigDict(extra="forbid")

    consent_version: Literal[2]
    events: list[BrowsingEventIn] = Field(min_length=1, max_length=50)


class BrowsingEventsAccepted(BaseModel):
    accepted: int


class UserActivityItem(BaseModel):
    id: str
    occurred_at: datetime
    source: Literal["browsing", "business", "login"]
    action: str
    details: dict[str, Any]


class UserActivityPage(BaseModel):
    items: list[UserActivityItem]
    has_more: bool
    last_activity_at: datetime | None

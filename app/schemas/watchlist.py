"""Watchlist request/response schemas."""
from datetime import datetime
from typing import Annotated, Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field

from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind


class WatchlistCreateRequest(BaseModel):
    name: str = Field(min_length=1, max_length=100)


class WatchlistEntryAddRequest(BaseModel):
    product_id: UUID
    delivery_point_id: UUID | None = None


class WatchlistEntryResponse(BaseModel):
    id: UUID
    product_id: UUID
    product_name: str | None = None
    delivery_point_id: UUID | None = None
    delivery_point_name: str | None = None
    best_bid: float | None = None
    best_ask: float | None = None
    created_at: datetime

    model_config = ConfigDict(from_attributes=True)


class WatchlistResponse(BaseModel):
    id: UUID
    name: str
    entry_count: int = 0
    entries: list[WatchlistEntryResponse] = Field(default_factory=list)
    created_at: datetime

    model_config = ConfigDict(from_attributes=True)


class SliceTargetCreate(BaseModel):
    target_type: Literal["SLICE"]
    market_product_code: str
    delivery_point_id: UUID
    availability_window_code: str


class PinTargetCreate(BaseModel):
    target_type: Literal["PIN"]
    order_id: UUID


WatchlistTargetCreate = Annotated[SliceTargetCreate | PinTargetCreate, Field(discriminator='target_type')]


class WatchlistTargetResponse(BaseModel):
    id: UUID
    target_type: Literal["SLICE", "PIN"]
    market_product_code: str | None = None
    delivery_point_id: UUID | None = None
    delivery_point_name: str | None = None
    availability_window_code: str | None = None
    order_id: UUID | None = None
    snapshot_price_per_mt_usd: float | None = None
    snapshot_quantity_mt: float | None = None
    snapshot_remaining_quantity_mt: float | None = None
    snapshot_status: str | None = None
    snapshot_side: str | None = None
    snapshot_market_product: str | None = None
    snapshot_delivery_point_name: str | None = None
    snapshot_availability_window: str | None = None
    snapshot_counterparty_label: str | None = None
    active_order_count: int = 0
    unread_event_count: int = 0
    latest_event_at: datetime | None = None
    created_at: datetime


class WatchlistSliceResponse(BaseModel):
    id: UUID
    target_type: Literal["SLICE"] = "SLICE"
    market_product_code: str
    delivery_point_id: UUID
    delivery_point_name: str | None = None
    availability_window_code: str
    active_order_count: int = 0
    unread_event_count: int = 0
    latest_event_at: datetime | None = None
    pins: list[WatchlistTargetResponse] = Field(default_factory=list)
    created_at: datetime


class WatchlistSummaryResponse(BaseModel):
    id: UUID
    name: str
    kind: str
    unread_event_count: int = 0
    latest_event_at: datetime | None = None
    total_slice_count: int = 0
    has_more_slices: bool = False
    slices: list[WatchlistSliceResponse] = Field(default_factory=list)
    created_at: datetime


class WatchlistDetailResponse(WatchlistSummaryResponse):
    pass


class WatchlistEventResponse(BaseModel):
    id: UUID
    watchlist_id: UUID
    watchlist_target_id: UUID
    target_type: Literal["SLICE", "PIN"]
    event_type: str
    event_payload: dict = Field(default_factory=dict)
    source_kind: MarketSourceKind = MarketSourceKind.UNKNOWN
    scope: MarketScope = MarketScope.UNKNOWN
    demo_status: MarketDemoStatus = MarketDemoStatus.UNKNOWN
    observed_at: datetime | None = None
    market_product_code: str | None = None
    delivery_point_id: UUID | None = None
    delivery_point_name: str | None = None
    availability_window_code: str | None = None
    order_id: UUID | None = None
    is_read: bool
    created_at: datetime


class WatchlistEventsPageResponse(BaseModel):
    items: list[WatchlistEventResponse] = Field(default_factory=list)
    next_cursor: str | None = None


class ProblemDetails(BaseModel):
    detail: str

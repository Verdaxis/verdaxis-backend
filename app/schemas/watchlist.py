"""Watchlist request/response schemas."""
from datetime import datetime
from typing import Optional
from uuid import UUID
from pydantic import BaseModel, ConfigDict, Field


class WatchlistCreateRequest(BaseModel):
    name: str = Field(min_length=1, max_length=100)


class WatchlistEntryAddRequest(BaseModel):
    product_id: UUID
    delivery_point_id: Optional[UUID] = None


class WatchlistEntryResponse(BaseModel):
    id: UUID
    product_id: UUID
    product_name: Optional[str] = None
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    created_at: datetime

    model_config = ConfigDict(from_attributes=True)


class WatchlistResponse(BaseModel):
    id: UUID
    name: str
    entry_count: int = 0
    entries: list[WatchlistEntryResponse] = []
    created_at: datetime

    model_config = ConfigDict(from_attributes=True)

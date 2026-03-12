"""
Pydantic schemas for the forward curve API.
"""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID

from pydantic import BaseModel


class ForwardCurvePoint(BaseModel):
    """
    A single point on the forward curve for one availability_window.

    best_bid = highest open BID price for this window
    best_ask = lowest open ASK price for this window
    mid_price = (best_bid + best_ask) / 2, None if only one side exists
    spread = best_ask - best_bid, None if only one side exists
    volume_mt = total remaining_quantity_mt across all OPEN/PARTIALLY_FILLED orders in this window
    order_count = total number of orders across both sides
    """

    availability_window: str
    best_bid: Optional[Decimal] = None
    best_ask: Optional[Decimal] = None
    mid_price: Optional[Decimal] = None
    spread: Optional[Decimal] = None
    volume_mt: Decimal
    order_count: int

    model_config = {"from_attributes": True}


class ForwardCurveResponse(BaseModel):
    """Response wrapper for the forward curve endpoint."""

    product_id: UUID
    delivery_point_id: Optional[UUID] = None
    curve: list[ForwardCurvePoint]
    generated_at: datetime

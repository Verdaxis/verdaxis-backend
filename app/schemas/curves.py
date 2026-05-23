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


class ForwardCurveBoardProduct(BaseModel):
    """Product identity used by the forward-curve monitoring board."""

    product_id: UUID
    market_product: str
    product_name: str


class ForwardCurveBoardDepthLevel(BaseModel):
    """Aggregated visible depth for one side of the focused market slice."""

    price_per_mt_usd: Decimal
    quantity_mt: Decimal
    order_count: int


class ForwardCurveBoardCell(BaseModel):
    """One product-port cell in the monitoring board matrix."""

    product_id: UUID
    market_product: str
    product_name: str
    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    availability_window: str
    benchmark_mid: Optional[Decimal] = None
    benchmark_source: Optional[str] = None
    is_demo_benchmark: bool = False
    best_bid: Optional[Decimal] = None
    best_ask: Optional[Decimal] = None
    spread: Optional[Decimal] = None
    volume_mt: Decimal = Decimal("0")
    order_count: int = 0


class ForwardCurveBoardPort(BaseModel):
    """Port row in the monitoring board matrix."""

    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    cells: list[ForwardCurveBoardCell]


class ForwardCurveBoardFocus(BaseModel):
    """Focused detail context selected from the board matrix."""

    product_id: UUID
    market_product: str
    product_name: str
    delivery_point_id: UUID
    delivery_point_name: str
    region: str
    availability_window: str
    curve: list[ForwardCurveBoardCell]
    depth_bids: list[ForwardCurveBoardDepthLevel]
    depth_asks: list[ForwardCurveBoardDepthLevel]


class ForwardCurveBoardResponse(BaseModel):
    """Aggregated board for the Forward Curve monitoring workspace."""

    availability_window: str
    products: list[ForwardCurveBoardProduct]
    ports: list[ForwardCurveBoardPort]
    focus: ForwardCurveBoardFocus
    generated_at: datetime

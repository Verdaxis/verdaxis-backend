"""
Forward Curve API.

Aggregates OPEN and PARTIALLY_FILLED orders by availability_window and side to
produce a forward price curve: best bid, best ask, mid-price, and spread per window.

Endpoints:
  GET /curves/forward         — JSON forward curve for a product
  GET /curves/forward/export  — CSV download of the same curve
"""
import csv
import io
from datetime import datetime, UTC
from decimal import Decimal
from typing import Optional
from uuid import UUID

from fastapi import APIRouter, Depends, Query
from fastapi.responses import StreamingResponse
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.schemas.curves import ForwardCurvePoint, ForwardCurveResponse
from app.services.availability_windows import availability_window_sort_key, normalize_availability_window


router = APIRouter(prefix="/curves/forward", tags=["forward-curve"])

_ACTIVE_STATUSES = [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]

# Canonical window ordering for deterministic response ordering
async def compute_forward_curve(
    db: AsyncSession,
    product_id: UUID,
    delivery_point_id: Optional[UUID] = None,
) -> list[ForwardCurvePoint]:
    """
    Query OPEN and PARTIALLY_FILLED orders grouped by (availability_window, side).

    For each window:
      - best_bid = MAX price among BID orders
      - best_ask = MIN price among ASK orders
      - mid_price = (best_bid + best_ask) / 2 if both sides present, else None
      - spread = best_ask - best_bid if both sides present, else None
      - volume_mt = SUM of remaining_quantity_mt across all orders in this window
      - order_count = COUNT of orders across both sides

    Returns ForwardCurvePoint list sorted by availability_window.
    """
    stmt = (
        select(
            OrderBookOrder.availability_window,
            OrderBookOrder.side,
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.min(OrderBookOrder.price_per_mt_usd).label("min_price"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_volume"),
            func.count(OrderBookOrder.id).label("order_count"),
        )
        .where(
            OrderBookOrder.status.in_(_ACTIVE_STATUSES),
            OrderBookOrder.product_id == product_id,
        )
        .group_by(OrderBookOrder.availability_window, OrderBookOrder.side)
    )

    if delivery_point_id is not None:
        stmt = stmt.where(OrderBookOrder.delivery_point_id == delivery_point_id)

    result = await db.execute(stmt)
    rows = result.all()

    # Aggregate rows into per-window buckets
    # key: availability_window → {"BID": row, "ASK": row}
    windows: dict[str, dict[str, object]] = {}
    for row in rows:
        window = normalize_availability_window(str(row.availability_window))
        if window not in windows:
            windows[window] = {}
        windows[window][row.side.value if hasattr(row.side, "value") else str(row.side)] = row

    points: list[ForwardCurvePoint] = []
    for window, sides in windows.items():
        bid_row = sides.get("BID")
        ask_row = sides.get("ASK")

        # BID: we want the HIGHEST bid (most aggressive buyer) = max_price
        best_bid: Optional[Decimal] = bid_row.max_price if bid_row else None
        # ASK: we want the LOWEST ask (most aggressive seller) = min_price
        best_ask: Optional[Decimal] = ask_row.min_price if ask_row else None

        mid_price: Optional[Decimal] = None
        spread: Optional[Decimal] = None
        if best_bid is not None and best_ask is not None:
            mid_price = Decimal(str(round((best_bid + best_ask) / 2, 2)))
            spread = Decimal(str(round(best_ask - best_bid, 2)))

        total_volume = Decimal("0")
        total_orders = 0
        for side_row in sides.values():
            total_volume += side_row.total_volume or Decimal("0")
            total_orders += side_row.order_count or 0

        points.append(
            ForwardCurvePoint(
                availability_window=window,
                best_bid=best_bid,
                best_ask=best_ask,
                mid_price=mid_price,
                spread=spread,
                volume_mt=total_volume,
                order_count=total_orders,
            )
        )

    points.sort(key=lambda p: availability_window_sort_key(p.availability_window))
    return points


def build_csv(points: list[ForwardCurvePoint]) -> str:
    """
    Serialise a list of ForwardCurvePoint objects to CSV text.

    None values are written as empty strings.
    """
    buf = io.StringIO()
    fieldnames = [
        "availability_window",
        "best_bid",
        "best_ask",
        "mid_price",
        "spread",
        "volume_mt",
        "order_count",
    ]
    writer = csv.DictWriter(buf, fieldnames=fieldnames, lineterminator="\n")
    writer.writeheader()
    for point in points:
        writer.writerow({
            "availability_window": point.availability_window,
            "best_bid": str(point.best_bid) if point.best_bid is not None else "",
            "best_ask": str(point.best_ask) if point.best_ask is not None else "",
            "mid_price": str(point.mid_price) if point.mid_price is not None else "",
            "spread": str(point.spread) if point.spread is not None else "",
            "volume_mt": str(point.volume_mt),
            "order_count": point.order_count,
        })
    return buf.getvalue()


@router.get("", response_model=ForwardCurveResponse, summary="Forward price curve for a product")
async def get_forward_curve(
    product_id: UUID = Query(..., description="Product UUID (required)"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter to a specific delivery point"),
    db: AsyncSession = Depends(get_db),
) -> ForwardCurveResponse:
    """
    Return the forward price curve for a product.

    For each availability_window with active orders the response includes:
    best bid, best ask, mid-price, spread, total remaining volume, and order count.
    """
    curve = await compute_forward_curve(db, product_id=product_id, delivery_point_id=delivery_point_id)
    return ForwardCurveResponse(
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        curve=curve,
        generated_at=datetime.now(UTC),
    )


@router.get("/export", summary="Download forward curve as CSV")
async def export_forward_curve(
    product_id: UUID = Query(..., description="Product UUID (required)"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter to a specific delivery point"),
    db: AsyncSession = Depends(get_db),
) -> StreamingResponse:
    """
    Stream the forward price curve as a CSV download.

    The response includes a Content-Disposition attachment header so browsers
    will prompt to save the file.
    """
    curve = await compute_forward_curve(db, product_id=product_id, delivery_point_id=delivery_point_id)
    csv_text = build_csv(curve)
    filename = f"forward_curve_{product_id}.csv"
    return StreamingResponse(
        iter([csv_text]),
        media_type="text/csv",
        headers={"Content-Disposition": f'attachment; filename="{filename}"'},
    )

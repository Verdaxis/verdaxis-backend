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
from datetime import datetime, timezone
from decimal import Decimal
from typing import Optional
from uuid import UUID

from fastapi import APIRouter, Depends, Query
from fastapi.responses import StreamingResponse
from sqlalchemy import case, select, func
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.catalog import DeliveryPoint, MarketProduct, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.schemas.curves import (
    ForwardCurveBoardCell,
    ForwardCurveBoardDepthLevel,
    ForwardCurveBoardFocus,
    ForwardCurveBoardPort,
    ForwardCurveBoardProduct,
    ForwardCurveBoardResponse,
    ForwardCurvePoint,
    ForwardCurveResponse,
)
from app.services.availability_windows import (
    SPOT_WINDOW,
    availability_window_sort_key,
    normalize_availability_window,
)
from app.services.benchmarks import get_benchmark_quote
from app.services.demo_market import DEMO_MARKET_ORG_IDS


router = APIRouter(prefix="/curves/forward", tags=["forward-curve"])

_ACTIVE_STATUSES = [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
_MARKET_PRODUCT_ORDER = [member.value for member in MarketProduct]
_DELIVERY_POINT_DISPLAY_ORDER = {
    "Dalian": 1,
    "Busan": 2,
    "Shanghai": 3,
    "Singapore": 4,
    "Rotterdam": 5,
    "Houston": 6,
    "Los Angeles": 7,
    "Santos": 8,
}


def _add_month_offset(year: int, month: int, offset: int) -> tuple[int, int]:
    zero_based = month - 1 + offset
    return year + (zero_based // 12), (zero_based % 12) + 1


def _default_curve_windows(now: datetime | None = None) -> list[str]:
    current = now or datetime.now(timezone.utc)
    year = current.year
    month = current.month
    quarter = ((month - 1) // 3) + 1

    windows = [SPOT_WINDOW]
    for offset in range(1, 7):
        next_year, next_month = _add_month_offset(year, month, offset)
        windows.append(f"{next_year}-{next_month:02d}")

    for offset in range(1, 5):
        absolute_quarter = (year * 4) + (quarter - 1) + offset
        quarter_year = absolute_quarter // 4
        next_quarter = (absolute_quarter % 4) + 1
        windows.append(f"{quarter_year}-Q{next_quarter}")

    windows.extend([f"{year + 1}-CAL", f"{year + 2}-CAL"])
    return windows


def _benchmark_is_demo(source: str | None) -> bool:
    return source in {None, "seed_matrix"}


def _bucket_has_real_orders(bucket: dict[str, object] | None) -> bool:
    return int(bucket.get("real_order_count") or 0) > 0 if bucket else False


def _money(value: Decimal | int | float | None) -> Decimal | None:
    if value is None:
        return None
    return Decimal(str(value)).quantize(Decimal("0.01"))

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


async def _load_board_products(db: AsyncSession) -> list[Product]:
    result = await db.execute(select(Product).where(Product.is_active.is_(True)))
    products = [
        product for product in result.scalars().all()
        if product.market_product in _MARKET_PRODUCT_ORDER
    ]
    products.sort(key=lambda product: _MARKET_PRODUCT_ORDER.index(product.market_product or ""))
    return products


async def _load_board_delivery_points(db: AsyncSession) -> list[DeliveryPoint]:
    display_order = case(
        _DELIVERY_POINT_DISPLAY_ORDER,
        value=DeliveryPoint.name,
        else_=999,
    )
    result = await db.execute(
        select(DeliveryPoint)
        .where(
            DeliveryPoint.is_active.is_(True),
            DeliveryPoint.name.in_(_DELIVERY_POINT_DISPLAY_ORDER.keys()),
        )
        .order_by(display_order, DeliveryPoint.name)
    )
    return result.scalars().all()


async def _aggregate_orderbook_window(
    db: AsyncSession,
    *,
    product_ids: list[UUID],
    delivery_point_ids: list[UUID],
    availability_window: str,
) -> dict[tuple[UUID, UUID], dict[str, object]]:
    if not product_ids or not delivery_point_ids:
        return {}

    stmt = (
        select(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.side,
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.min(OrderBookOrder.price_per_mt_usd).label("min_price"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_volume"),
            func.count(OrderBookOrder.id).label("order_count"),
            func.sum(
                case(
                    (OrderBookOrder.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)), 1),
                    else_=0,
                )
            ).label("real_order_count"),
        )
        .where(
            OrderBookOrder.status.in_(_ACTIVE_STATUSES),
            OrderBookOrder.product_id.in_(product_ids),
            OrderBookOrder.delivery_point_id.in_(delivery_point_ids),
            OrderBookOrder.availability_window == availability_window,
        )
        .group_by(OrderBookOrder.product_id, OrderBookOrder.delivery_point_id, OrderBookOrder.side)
    )

    result = await db.execute(stmt)
    board: dict[tuple[UUID, UUID], dict[str, object]] = {}
    for row in result.all():
        if row.delivery_point_id is None:
            continue
        key = (row.product_id, row.delivery_point_id)
        bucket = board.setdefault(key, {"volume_mt": Decimal("0"), "order_count": 0})
        side = row.side.value if hasattr(row.side, "value") else str(row.side)
        if side == OrderSide.BID.value:
            bucket["best_bid"] = row.max_price
        elif side == OrderSide.ASK.value:
            bucket["best_ask"] = row.min_price
        bucket["volume_mt"] = Decimal(str(bucket["volume_mt"])) + (row.total_volume or Decimal("0"))
        bucket["order_count"] = int(bucket["order_count"]) + int(row.order_count or 0)
        bucket["real_order_count"] = int(bucket.get("real_order_count") or 0) + int(row.real_order_count or 0)
    return board


def _orderbook_bucket_to_values(bucket: dict[str, object] | None) -> tuple[Decimal | None, Decimal | None, Decimal | None, Decimal, int]:
    if not bucket:
        return None, None, None, Decimal("0"), 0

    best_bid = _money(bucket.get("best_bid"))
    best_ask = _money(bucket.get("best_ask"))
    spread = _money(best_ask - best_bid) if best_bid is not None and best_ask is not None else None
    volume_mt = _money(bucket.get("volume_mt")) or Decimal("0")
    order_count = int(bucket.get("order_count") or 0)
    return best_bid, best_ask, spread, volume_mt, order_count


async def _build_board_cell(
    db: AsyncSession,
    *,
    product: Product,
    delivery_point: DeliveryPoint,
    availability_window: str,
    orderbook_bucket: dict[str, object] | None,
) -> ForwardCurveBoardCell:
    quote = await get_benchmark_quote(
        db,
        market_product=product.market_product,
        delivery_point_id=delivery_point.id,
        availability_window=availability_window,
    )
    best_bid, best_ask, spread, volume_mt, order_count = _orderbook_bucket_to_values(orderbook_bucket)
    benchmark_mid = _money(quote.benchmark_price_per_mt_usd) if quote else None
    benchmark_source = quote.source if quote else None
    if _benchmark_is_demo(benchmark_source) and _bucket_has_real_orders(orderbook_bucket):
        benchmark_mid = None
        benchmark_source = None
    benchmark_is_demo = benchmark_mid is not None and _benchmark_is_demo(benchmark_source)

    return ForwardCurveBoardCell(
        product_id=product.id,
        market_product=product.market_product or "",
        product_name=product.name,
        delivery_point_id=delivery_point.id,
        delivery_point_name=delivery_point.name,
        region=delivery_point.region,
        availability_window=availability_window,
        benchmark_mid=benchmark_mid,
        benchmark_source=benchmark_source,
        is_demo_benchmark=benchmark_is_demo,
        best_bid=best_bid,
        best_ask=best_ask,
        spread=spread,
        volume_mt=volume_mt,
        order_count=order_count,
    )


async def _aggregate_depth_levels(
    db: AsyncSession,
    *,
    product_id: UUID,
    delivery_point_id: UUID,
    availability_window: str,
) -> tuple[list[ForwardCurveBoardDepthLevel], list[ForwardCurveBoardDepthLevel]]:
    stmt = (
        select(
            OrderBookOrder.side,
            OrderBookOrder.price_per_mt_usd,
            func.sum(OrderBookOrder.remaining_quantity_mt).label("quantity_mt"),
            func.count(OrderBookOrder.id).label("order_count"),
        )
        .where(
            OrderBookOrder.status.in_(_ACTIVE_STATUSES),
            OrderBookOrder.product_id == product_id,
            OrderBookOrder.delivery_point_id == delivery_point_id,
            OrderBookOrder.availability_window == availability_window,
        )
        .group_by(OrderBookOrder.side, OrderBookOrder.price_per_mt_usd)
    )
    result = await db.execute(stmt)
    bids: list[ForwardCurveBoardDepthLevel] = []
    asks: list[ForwardCurveBoardDepthLevel] = []
    for row in result.all():
        level = ForwardCurveBoardDepthLevel(
            price_per_mt_usd=_money(row.price_per_mt_usd) or Decimal("0"),
            quantity_mt=_money(row.quantity_mt) or Decimal("0"),
            order_count=int(row.order_count or 0),
        )
        side = row.side.value if hasattr(row.side, "value") else str(row.side)
        if side == OrderSide.BID.value:
            bids.append(level)
        elif side == OrderSide.ASK.value:
            asks.append(level)

    bids.sort(key=lambda level: level.price_per_mt_usd, reverse=True)
    asks.sort(key=lambda level: level.price_per_mt_usd)
    return bids[:10], asks[:10]


async def build_forward_curve_board(
    db: AsyncSession,
    *,
    availability_window: str = SPOT_WINDOW,
    focus_market_product: MarketProduct | None = None,
    focus_delivery_point_id: UUID | None = None,
) -> ForwardCurveBoardResponse:
    normalized_window = normalize_availability_window(availability_window)
    products = await _load_board_products(db)
    delivery_points = await _load_board_delivery_points(db)
    if not products or not delivery_points:
        raise ValueError("Forward curve board requires active products and delivery points")

    product_ids = [product.id for product in products]
    delivery_point_ids = [point.id for point in delivery_points]
    orderbook_by_key = await _aggregate_orderbook_window(
        db,
        product_ids=product_ids,
        delivery_point_ids=delivery_point_ids,
        availability_window=normalized_window,
    )

    ports: list[ForwardCurveBoardPort] = []
    for delivery_point in delivery_points:
        cells = [
            await _build_board_cell(
                db,
                product=product,
                delivery_point=delivery_point,
                availability_window=normalized_window,
                orderbook_bucket=orderbook_by_key.get((product.id, delivery_point.id)),
            )
            for product in products
        ]
        ports.append(
            ForwardCurveBoardPort(
                delivery_point_id=delivery_point.id,
                delivery_point_name=delivery_point.name,
                region=delivery_point.region,
                cells=cells,
            )
        )

    normalized_focus_market_product = (
        focus_market_product.value
        if isinstance(focus_market_product, MarketProduct)
        else str(focus_market_product or MarketProduct.BIO_METHANOL.value)
    )
    focus_product = next(
        (product for product in products if product.market_product == normalized_focus_market_product),
        products[0],
    )
    focus_delivery_point = next(
        (point for point in delivery_points if point.id == focus_delivery_point_id),
        next((point for point in delivery_points if point.name == "Singapore"), delivery_points[0]),
    )

    orderbook_curve = {
        point.availability_window: point
        for point in await compute_forward_curve(
            db,
            product_id=focus_product.id,
            delivery_point_id=focus_delivery_point.id,
        )
    }
    curve_windows = sorted(
        set(_default_curve_windows()) | set(orderbook_curve.keys()),
        key=availability_window_sort_key,
    )
    focus_curve: list[ForwardCurveBoardCell] = []
    for window in curve_windows:
        point = orderbook_curve.get(window)
        bucket = None
        if point is not None:
            bucket = {
                "best_bid": point.best_bid,
                "best_ask": point.best_ask,
                "volume_mt": point.volume_mt,
                "order_count": point.order_count,
            }
        focus_curve.append(
            await _build_board_cell(
                db,
                product=focus_product,
                delivery_point=focus_delivery_point,
                availability_window=window,
                orderbook_bucket=bucket,
            )
        )

    depth_bids, depth_asks = await _aggregate_depth_levels(
        db,
        product_id=focus_product.id,
        delivery_point_id=focus_delivery_point.id,
        availability_window=normalized_window,
    )

    focus = ForwardCurveBoardFocus(
        product_id=focus_product.id,
        market_product=focus_product.market_product or "",
        product_name=focus_product.name,
        delivery_point_id=focus_delivery_point.id,
        delivery_point_name=focus_delivery_point.name,
        region=focus_delivery_point.region,
        availability_window=normalized_window,
        curve=focus_curve,
        depth_bids=depth_bids,
        depth_asks=depth_asks,
    )

    return ForwardCurveBoardResponse(
        availability_window=normalized_window,
        products=[
            ForwardCurveBoardProduct(
                product_id=product.id,
                market_product=product.market_product or "",
                product_name=product.name,
            )
            for product in products
        ],
        ports=ports,
        focus=focus,
        generated_at=datetime.now(timezone.utc),
    )


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


@router.get("/board", response_model=ForwardCurveBoardResponse, summary="Forward curve monitoring board")
async def get_forward_curve_board(
    availability_window: str = Query(SPOT_WINDOW, description="Matrix availability window"),
    focus_market_product: Optional[MarketProduct] = Query(None, description="Focused canonical market product"),
    focus_delivery_point_id: Optional[UUID] = Query(None, description="Focused delivery point"),
    db: AsyncSession = Depends(get_db),
) -> ForwardCurveBoardResponse:
    """
    Return the aggregated Forward Curve monitoring board.

    The matrix is all approved delivery points by all approved market products for
    one selected availability window. The focus section returns the hybrid
    benchmark/orderbook curve and depth for the selected product-port context.
    """
    return await build_forward_curve_board(
        db,
        availability_window=availability_window,
        focus_market_product=focus_market_product,
        focus_delivery_point_id=focus_delivery_point_id,
    )


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
        generated_at=datetime.now(timezone.utc),
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

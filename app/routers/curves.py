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
from types import SimpleNamespace
from typing import Optional
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query
from fastapi.responses import StreamingResponse
from sqlalchemy import case, select, func, or_
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.market_catalog import (
    DELIVERY_POINT_DISPLAY_ORDER,
    MARKET_PRODUCT_CODES,
    MarketProduct,
)
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrganizationProvenance
from app.schemas.benchmark import BenchmarkQuote
from app.schemas.curves import (
    ForwardCurveBoardCell,
    ForwardCurveBoardDepthLevel,
    ForwardCurveBoardFairPriceBand,
    ForwardCurveBoardFocus,
    ForwardCurveBoardIndicationSummary,
    ForwardCurveBoardPort,
    ForwardCurveBoardProduct,
    ForwardCurveBoardPhysicalStemSummary,
    ForwardCurveBoardResponse,
    ForwardCurveSliceResponse,
    ForwardCurveTableResponse,
    ForwardCurveSignalProvenance,
    ForwardCurvePoint,
    ForwardCurveResponse,
    MarketSignalType,
)
from app.schemas.market_activity import (
    MarketScope,
    MarketSourceKind,
)
from app.services.availability_windows import (
    SPOT_WINDOW,
    availability_window_sort_key,
    normalize_availability_window,
)
from app.services.benchmarks import get_benchmark_quote, get_benchmark_quotes
from app.services.market_provenance import (
    MarketEvidenceScope,
    evidence_policy_for_scope,
    order_evidence_clause,
    public_order_evidence_clause,
    select_aggregate_evidence,
)
from app.services.market_data_eligibility import (
    canonical_delivery_point_clause,
    canonical_product_clause,
    current_public_order_clause,
)
from app.services.forward_monitoring import (
    SignalKey,
    load_fair_price_bands,
    load_indication_summaries,
    load_latest_indications_for_focus,
    load_physical_stem_summaries,
    load_physical_stems_for_focus,
    no_data_fair_price_band_provenance,
    no_data_summary_for_signal,
    normalize_signal_keys,
)
from app.services.forward_curve_market_slices import forward_curve_market_slices


router = APIRouter(prefix="/curves/forward", tags=["forward-curve"])

_ACTIVE_STATUSES = [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
_MARKET_PRODUCT_ORDER = list(MARKET_PRODUCT_CODES)
_DELIVERY_POINT_DISPLAY_ORDER = DELIVERY_POINT_DISPLAY_ORDER


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


_MISSING_BENCHMARK = object()


def _is_demo_order_clause():
    return order_evidence_clause(OrderBookOrder.provenance, MarketEvidenceScope.DEMO)


def _is_real_order_clause():
    return order_evidence_clause(OrderBookOrder.provenance, MarketEvidenceScope.REAL)


def _source_kind_for_best_price(
    *,
    best_price: Decimal | None,
    real_best: Decimal | None,
    demo_best: Decimal | None,
) -> MarketSourceKind:
    if best_price is None:
        return MarketSourceKind.NO_DATA

    real_matches = real_best is not None and _money(real_best) == _money(best_price)
    demo_matches = demo_best is not None and _money(demo_best) == _money(best_price)
    if real_matches and demo_matches:
        return MarketSourceKind.MIXED_SOURCE
    if demo_matches:
        return MarketSourceKind.DEMO_SEED
    if real_matches:
        return MarketSourceKind.LIVE_ORDER
    return MarketSourceKind.UNKNOWN


def _source_kind_for_benchmark(quote: BenchmarkQuote | None, *, benchmark_is_demo: bool) -> MarketSourceKind:
    if quote is None:
        return MarketSourceKind.NO_DATA
    if benchmark_is_demo:
        return MarketSourceKind.DEMO_SEED
    return MarketSourceKind.BENCHMARK_REFERENCE

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
            current_public_order_clause(OrderBookOrder),
            OrderBookOrder.product_id == product_id,
            # Real orders only: sibling endpoints (/table, /slice, /board)
            # segregate and label demo liquidity; this legacy curve has no
            # source labelling, so demo orders must not leak into it.
            _is_real_order_clause(),
        )
        .group_by(OrderBookOrder.availability_window, OrderBookOrder.side)
    )

    if delivery_point_id is not None:
        stmt = stmt.where(OrderBookOrder.delivery_point_id == delivery_point_id)

    result = await db.execute(stmt)
    rows = result.all()

    # Aggregate rows into per-window buckets, merging rows whose raw
    # availability_window aliases normalize to the same canonical window
    # (SQL groups by the raw value, so aliases arrive as separate rows).
    # key: availability_window -> {"BID": agg, "ASK": agg}
    windows: dict[str, dict[str, SimpleNamespace]] = {}
    for row in rows:
        window = normalize_availability_window(str(row.availability_window))
        side = row.side.value if hasattr(row.side, "value") else str(row.side)
        agg = windows.setdefault(window, {}).get(side)
        if agg is None:
            windows[window][side] = SimpleNamespace(
                max_price=row.max_price,
                min_price=row.min_price,
                total_volume=row.total_volume or Decimal("0"),
                order_count=row.order_count or 0,
            )
        else:
            if row.max_price is not None:
                agg.max_price = row.max_price if agg.max_price is None else max(agg.max_price, row.max_price)
            if row.min_price is not None:
                agg.min_price = row.min_price if agg.min_price is None else min(agg.min_price, row.min_price)
            agg.total_volume += row.total_volume or Decimal("0")
            agg.order_count += row.order_count or 0

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
    result = await db.execute(
        select(Product).where(
            canonical_product_clause(Product),
        )
    )
    products = list(result.scalars().all())
    products.sort(key=lambda product: _MARKET_PRODUCT_ORDER.index(product.market_product or ""))
    return products


async def _load_board_delivery_points(db: AsyncSession) -> list[DeliveryPoint]:
    display_order = case(
        dict(_DELIVERY_POINT_DISPLAY_ORDER),
        value=DeliveryPoint.name,
        else_=999,
    )
    result = await db.execute(
        select(DeliveryPoint)
        .where(
            canonical_delivery_point_clause(DeliveryPoint),
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

    demo_order = _is_demo_order_clause()
    real_order = _is_real_order_clause()
    unknown_order = OrderBookOrder.provenance == OrganizationProvenance.UNKNOWN.value
    observed_at = func.coalesce(OrderBookOrder.updated_at, OrderBookOrder.created_at)
    stmt = (
        select(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.side,
            func.max(case((real_order, OrderBookOrder.price_per_mt_usd))).label("real_max_price"),
            func.min(case((real_order, OrderBookOrder.price_per_mt_usd))).label("real_min_price"),
            func.max(case((demo_order, OrderBookOrder.price_per_mt_usd))).label("demo_max_price"),
            func.min(case((demo_order, OrderBookOrder.price_per_mt_usd))).label("demo_min_price"),
            func.sum(case((real_order, OrderBookOrder.remaining_quantity_mt), else_=0)).label("real_volume_mt"),
            func.sum(case((demo_order, OrderBookOrder.remaining_quantity_mt), else_=0)).label("demo_volume_mt"),
            func.sum(case((real_order, 1), else_=0)).label("real_order_count"),
            func.sum(case((demo_order, 1), else_=0)).label("demo_order_count"),
            func.sum(case((unknown_order, 1), else_=0)).label("unknown_order_count"),
            func.max(case((real_order, observed_at))).label("real_last_order_at"),
            func.max(case((demo_order, observed_at))).label("demo_last_order_at"),
        )
        .where(
            OrderBookOrder.status.in_(_ACTIVE_STATUSES),
            current_public_order_clause(OrderBookOrder),
            OrderBookOrder.product_id.in_(product_ids),
            OrderBookOrder.delivery_point_id.in_(delivery_point_ids),
            OrderBookOrder.availability_window == availability_window,
            or_(public_order_evidence_clause(OrderBookOrder.provenance), unknown_order),
        )
        .group_by(OrderBookOrder.product_id, OrderBookOrder.delivery_point_id, OrderBookOrder.side)
    )

    result = await db.execute(stmt)
    board: dict[tuple[UUID, UUID], dict[str, object]] = {}
    for row in result.all():
        if row.delivery_point_id is None:
            continue
        key = (row.product_id, row.delivery_point_id)
        bucket = board.setdefault(
            key,
            {"real_volume_mt": Decimal("0"), "demo_volume_mt": Decimal("0")},
        )
        side = row.side.value if hasattr(row.side, "value") else str(row.side)
        if side == OrderSide.BID.value:
            bucket["real_best_bid"] = row.real_max_price
            bucket["demo_best_bid"] = row.demo_max_price
        elif side == OrderSide.ASK.value:
            bucket["real_best_ask"] = row.real_min_price
            bucket["demo_best_ask"] = row.demo_min_price
        bucket["real_volume_mt"] = Decimal(str(bucket["real_volume_mt"])) + (row.real_volume_mt or Decimal("0"))
        bucket["demo_volume_mt"] = Decimal(str(bucket["demo_volume_mt"])) + (row.demo_volume_mt or Decimal("0"))
        bucket["real_order_count"] = int(bucket.get("real_order_count") or 0) + int(row.real_order_count or 0)
        bucket["demo_order_count"] = int(bucket.get("demo_order_count") or 0) + int(row.demo_order_count or 0)
        bucket["unknown_order_count"] = int(bucket.get("unknown_order_count") or 0) + int(row.unknown_order_count or 0)
        for prefix in ("real", "demo"):
            candidate = getattr(row, f"{prefix}_last_order_at")
            current = bucket.get(f"{prefix}_last_order_at")
            if candidate is not None and (current is None or candidate > current):
                bucket[f"{prefix}_last_order_at"] = candidate
    return board


async def _aggregate_orderbook_focus_windows(
    db: AsyncSession,
    *,
    product_id: UUID,
    delivery_point_id: UUID,
) -> dict[str, dict[str, object]]:
    demo_order = _is_demo_order_clause()
    real_order = _is_real_order_clause()
    unknown_order = OrderBookOrder.provenance == OrganizationProvenance.UNKNOWN.value
    observed_at = func.coalesce(OrderBookOrder.updated_at, OrderBookOrder.created_at)
    stmt = (
        select(
            OrderBookOrder.availability_window,
            OrderBookOrder.side,
            func.max(case((real_order, OrderBookOrder.price_per_mt_usd))).label("real_max_price"),
            func.min(case((real_order, OrderBookOrder.price_per_mt_usd))).label("real_min_price"),
            func.max(case((demo_order, OrderBookOrder.price_per_mt_usd))).label("demo_max_price"),
            func.min(case((demo_order, OrderBookOrder.price_per_mt_usd))).label("demo_min_price"),
            func.sum(case((real_order, OrderBookOrder.remaining_quantity_mt), else_=0)).label("real_volume_mt"),
            func.sum(case((demo_order, OrderBookOrder.remaining_quantity_mt), else_=0)).label("demo_volume_mt"),
            func.sum(case((real_order, 1), else_=0)).label("real_order_count"),
            func.sum(case((demo_order, 1), else_=0)).label("demo_order_count"),
            func.sum(case((unknown_order, 1), else_=0)).label("unknown_order_count"),
            func.max(case((real_order, observed_at))).label("real_last_order_at"),
            func.max(case((demo_order, observed_at))).label("demo_last_order_at"),
        )
        .where(
            OrderBookOrder.status.in_(_ACTIVE_STATUSES),
            current_public_order_clause(OrderBookOrder),
            OrderBookOrder.product_id == product_id,
            OrderBookOrder.delivery_point_id == delivery_point_id,
            or_(public_order_evidence_clause(OrderBookOrder.provenance), unknown_order),
        )
        .group_by(OrderBookOrder.availability_window, OrderBookOrder.side)
    )

    result = await db.execute(stmt)
    buckets: dict[str, dict[str, object]] = {}
    for row in result.all():
        window = normalize_availability_window(str(row.availability_window))
        bucket = buckets.setdefault(
            window,
            {"real_volume_mt": Decimal("0"), "demo_volume_mt": Decimal("0")},
        )
        side = row.side.value if hasattr(row.side, "value") else str(row.side)
        if side == OrderSide.BID.value:
            bucket["real_best_bid"] = row.real_max_price
            bucket["demo_best_bid"] = row.demo_max_price
        elif side == OrderSide.ASK.value:
            bucket["real_best_ask"] = row.real_min_price
            bucket["demo_best_ask"] = row.demo_min_price
        bucket["real_volume_mt"] = Decimal(str(bucket["real_volume_mt"])) + (row.real_volume_mt or Decimal("0"))
        bucket["demo_volume_mt"] = Decimal(str(bucket["demo_volume_mt"])) + (row.demo_volume_mt or Decimal("0"))
        bucket["real_order_count"] = int(bucket.get("real_order_count") or 0) + int(row.real_order_count or 0)
        bucket["demo_order_count"] = int(bucket.get("demo_order_count") or 0) + int(row.demo_order_count or 0)
        bucket["unknown_order_count"] = int(bucket.get("unknown_order_count") or 0) + int(row.unknown_order_count or 0)
        for prefix in ("real", "demo"):
            candidate = getattr(row, f"{prefix}_last_order_at")
            current = bucket.get(f"{prefix}_last_order_at")
            if candidate is not None and (current is None or candidate > current):
                bucket[f"{prefix}_last_order_at"] = candidate
    return buckets


def _orderbook_bucket_to_values(bucket: dict[str, object] | None) -> tuple[Decimal | None, Decimal | None, Decimal | None, Decimal, int]:
    if not bucket:
        return None, None, None, Decimal("0"), 0

    selection = select_aggregate_evidence(
        real_count=int(bucket.get("real_order_count") or 0),
        demo_count=int(bucket.get("demo_order_count") or 0),
        unknown_count=int(bucket.get("unknown_order_count") or 0),
        real_source=MarketSourceKind.LIVE_ORDER,
    )
    if selection.value_prefix is None:
        return None, None, None, Decimal("0"), 0
    prefix = selection.value_prefix
    best_bid = _money(bucket.get(f"{prefix}_best_bid"))
    best_ask = _money(bucket.get(f"{prefix}_best_ask"))
    spread = _money(best_ask - best_bid) if best_bid is not None and best_ask is not None else None
    volume_mt = _money(bucket.get(f"{prefix}_volume_mt")) or Decimal("0")
    order_count = int(bucket.get(f"{prefix}_order_count") or 0)
    return best_bid, best_ask, spread, volume_mt, order_count


async def _build_board_cell(
    db: AsyncSession,
    *,
    product: Product,
    delivery_point: DeliveryPoint,
    availability_window: str,
    orderbook_bucket: dict[str, object] | None,
    benchmark_quote: BenchmarkQuote | None | object = _MISSING_BENCHMARK,
    indication_summary: ForwardCurveBoardIndicationSummary | None = None,
    fair_price_band: ForwardCurveBoardFairPriceBand | None = None,
    fair_price_band_provenance: ForwardCurveSignalProvenance | None = None,
    physical_stem_summary: ForwardCurveBoardPhysicalStemSummary | None = None,
) -> ForwardCurveBoardCell:
    quote = (
        await get_benchmark_quote(
            db,
            market_product=product.market_product,
            delivery_point_id=delivery_point.id,
            availability_window=availability_window,
        )
        if benchmark_quote is _MISSING_BENCHMARK
        else benchmark_quote
    )
    best_bid, best_ask, spread, volume_mt, order_count = _orderbook_bucket_to_values(orderbook_bucket)
    benchmark_mid = _money(quote.benchmark_price_per_mt_usd) if quote else None
    benchmark_source = quote.source if quote else None
    if _benchmark_is_demo(benchmark_source) and _bucket_has_real_orders(orderbook_bucket):
        benchmark_mid = None
        benchmark_source = None
    benchmark_is_demo = benchmark_mid is not None and _benchmark_is_demo(benchmark_source)
    real_order_count = int(orderbook_bucket.get("real_order_count") or 0) if orderbook_bucket else 0
    demo_order_count = int(orderbook_bucket.get("demo_order_count") or 0) if orderbook_bucket else 0
    unknown_order_count = int(orderbook_bucket.get("unknown_order_count") or 0) if orderbook_bucket else 0
    selection = select_aggregate_evidence(
        real_count=real_order_count,
        demo_count=demo_order_count,
        unknown_count=unknown_order_count,
        real_source=MarketSourceKind.LIVE_ORDER,
    )
    selected_scope = selection.scope
    demo_status = selection.demo_status
    order_source_kind = selection.source_kind
    real_best_bid = _money(orderbook_bucket.get("real_best_bid")) if orderbook_bucket else None
    real_best_ask = _money(orderbook_bucket.get("real_best_ask")) if orderbook_bucket else None
    demo_best_bid = _money(orderbook_bucket.get("demo_best_bid")) if orderbook_bucket else None
    demo_best_ask = _money(orderbook_bucket.get("demo_best_ask")) if orderbook_bucket else None

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
        order_source_kind=order_source_kind,
        benchmark_source_kind=_source_kind_for_benchmark(quote if benchmark_mid is not None else None, benchmark_is_demo=benchmark_is_demo),
        scope=MarketScope.DELIVERY_POINT,
        demo_status=demo_status,
        real_order_count=real_order_count,
        demo_order_count=demo_order_count,
        unknown_order_count=unknown_order_count,
        real_best_bid=real_best_bid,
        real_best_ask=real_best_ask,
        demo_best_bid=demo_best_bid,
        demo_best_ask=demo_best_ask,
        best_bid_source_kind=_source_kind_for_best_price(
            best_price=best_bid,
            real_best=real_best_bid if selected_scope == MarketEvidenceScope.REAL else None,
            demo_best=demo_best_bid if selected_scope == MarketEvidenceScope.DEMO else None,
        ),
        best_ask_source_kind=_source_kind_for_best_price(
            best_price=best_ask,
            real_best=real_best_ask if selected_scope == MarketEvidenceScope.REAL else None,
            demo_best=demo_best_ask if selected_scope == MarketEvidenceScope.DEMO else None,
        ),
        order_observed_at=(
            orderbook_bucket.get(
                "real_last_order_at"
                if selected_scope == MarketEvidenceScope.REAL
                else "demo_last_order_at"
            )
            if orderbook_bucket and selected_scope is not None
            else None
        ),
        benchmark_observed_at=getattr(quote, "observed_at", None) if quote and benchmark_mid is not None else None,
        indication_summary=indication_summary
        or no_data_summary_for_signal(MarketSignalType.MARKET_INDICATION),
        fair_price_band=fair_price_band,
        fair_price_band_provenance=fair_price_band_provenance
        or no_data_fair_price_band_provenance(),
        physical_stem_summary=physical_stem_summary
        or no_data_summary_for_signal(MarketSignalType.PHYSICAL_STEM),
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
            OrderBookOrder.provenance,
            OrderBookOrder.price_per_mt_usd,
            func.sum(OrderBookOrder.remaining_quantity_mt).label("quantity_mt"),
            func.count(OrderBookOrder.id).label("order_count"),
            func.sum(case((_is_real_order_clause(), 1), else_=0)).label("real_order_count"),
            func.sum(case((_is_demo_order_clause(), 1), else_=0)).label("demo_order_count"),
        )
        .where(
            OrderBookOrder.status.in_(_ACTIVE_STATUSES),
            current_public_order_clause(OrderBookOrder),
            OrderBookOrder.product_id == product_id,
            OrderBookOrder.delivery_point_id == delivery_point_id,
            OrderBookOrder.availability_window == availability_window,
            public_order_evidence_clause(OrderBookOrder.provenance),
        )
        .group_by(
            OrderBookOrder.side,
            OrderBookOrder.provenance,
            OrderBookOrder.price_per_mt_usd,
        )
    )
    result = await db.execute(stmt)
    rows = result.all()
    use_real = any(int(row.real_order_count or 0) > 0 for row in rows)
    bids: list[ForwardCurveBoardDepthLevel] = []
    asks: list[ForwardCurveBoardDepthLevel] = []
    for row in rows:
        real_order_count = int(row.real_order_count or 0)
        demo_order_count = int(row.demo_order_count or 0)
        if use_real and real_order_count == 0:
            continue
        if not use_real and demo_order_count == 0:
            continue
        evidence_scope = (
            MarketEvidenceScope.REAL if use_real else MarketEvidenceScope.DEMO
        )
        policy = evidence_policy_for_scope(
            evidence_scope,
            real_source=MarketSourceKind.LIVE_ORDER,
        )
        level = ForwardCurveBoardDepthLevel(
            price_per_mt_usd=_money(row.price_per_mt_usd) or Decimal("0"),
            quantity_mt=_money(row.quantity_mt) or Decimal("0"),
            order_count=int(row.order_count or 0),
            source_kind=policy.source_kind,
            demo_status=policy.demo_status,
            real_order_count=real_order_count,
            demo_order_count=demo_order_count,
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

    product_ids = [product.id for product in products]
    delivery_point_ids = [point.id for point in delivery_points]
    orderbook_by_key = await _aggregate_orderbook_window(
        db,
        product_ids=product_ids,
        delivery_point_ids=delivery_point_ids,
        availability_window=normalized_window,
    )
    focus_orderbook_by_window = await _aggregate_orderbook_focus_windows(
        db,
        product_id=focus_product.id,
        delivery_point_id=focus_delivery_point.id,
    )
    curve_windows = sorted(
        set(_default_curve_windows()) | set(focus_orderbook_by_window.keys()),
        key=availability_window_sort_key,
    )
    matrix_signal_keys: list[SignalKey] = [
        (product.market_product or "", delivery_point.id, normalized_window)
        for delivery_point in delivery_points
        for product in products
    ]
    focus_signal_keys: list[SignalKey] = [
        (focus_product.market_product or "", focus_delivery_point.id, window)
        for window in curve_windows
    ]
    signal_keys = normalize_signal_keys([*matrix_signal_keys, *focus_signal_keys])
    indication_summaries = await load_indication_summaries(db, signal_keys)
    physical_stem_summaries = await load_physical_stem_summaries(db, signal_keys)
    fair_price_bands = await load_fair_price_bands(db, signal_keys)
    benchmark_requests = [
        (product.market_product, delivery_point.id, normalized_window, delivery_point.name)
        for delivery_point in delivery_points
        for product in products
    ]
    benchmark_requests.extend(
        (focus_product.market_product, focus_delivery_point.id, window, focus_delivery_point.name)
        for window in curve_windows
    )
    benchmark_quotes = await get_benchmark_quotes(db, benchmark_requests)

    ports: list[ForwardCurveBoardPort] = []
    for delivery_point in delivery_points:
        cells: list[ForwardCurveBoardCell] = []
        for product in products:
            cell_key = (product.market_product or "", delivery_point.id, normalized_window)
            cell_fair_price_band = fair_price_bands.get(cell_key)
            cell = await _build_board_cell(
                db,
                product=product,
                delivery_point=delivery_point,
                availability_window=normalized_window,
                orderbook_bucket=orderbook_by_key.get((product.id, delivery_point.id)),
                benchmark_quote=benchmark_quotes.get(
                    (
                        product.market_product or "",
                        delivery_point.id,
                        normalized_window,
                    )
                ),
                indication_summary=indication_summaries.get(cell_key),
                fair_price_band=cell_fair_price_band,
                fair_price_band_provenance=(
                    cell_fair_price_band.provenance
                    if cell_fair_price_band
                    else no_data_fair_price_band_provenance()
                ),
                physical_stem_summary=physical_stem_summaries.get(cell_key),
            )
            cells.append(cell)
        ports.append(
            ForwardCurveBoardPort(
                delivery_point_id=delivery_point.id,
                delivery_point_name=delivery_point.name,
                region=delivery_point.region,
                cells=cells,
            )
        )

    focus_curve: list[ForwardCurveBoardCell] = []
    for window in curve_windows:
        focus_key = (focus_product.market_product or "", focus_delivery_point.id, window)
        focus_fair_price_band = fair_price_bands.get(focus_key)
        focus_curve.append(
            await _build_board_cell(
                db,
                product=focus_product,
                delivery_point=focus_delivery_point,
                availability_window=window,
                orderbook_bucket=focus_orderbook_by_window.get(window),
                benchmark_quote=benchmark_quotes.get(
                    (
                        focus_product.market_product or "",
                        focus_delivery_point.id,
                        window,
                    )
                ),
                indication_summary=indication_summaries.get(focus_key),
                fair_price_band=focus_fair_price_band,
                fair_price_band_provenance=(
                    focus_fair_price_band.provenance
                    if focus_fair_price_band
                    else no_data_fair_price_band_provenance()
                ),
                physical_stem_summary=physical_stem_summaries.get(focus_key),
            )
        )

    depth_bids, depth_asks = await _aggregate_depth_levels(
        db,
        product_id=focus_product.id,
        delivery_point_id=focus_delivery_point.id,
        availability_window=normalized_window,
    )
    focus_indications = await load_latest_indications_for_focus(
        db,
        market_product=focus_product.market_product or "",
        delivery_point_id=focus_delivery_point.id,
        availability_window=normalized_window,
    )
    focus_physical_stems = await load_physical_stems_for_focus(
        db,
        market_product=focus_product.market_product or "",
        delivery_point_id=focus_delivery_point.id,
        availability_window=normalized_window,
    )
    selected_focus_key = (focus_product.market_product or "", focus_delivery_point.id, normalized_window)
    selected_focus_fair_price_band = fair_price_bands.get(selected_focus_key)

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
        indications=focus_indications,
        fair_price_band=selected_focus_fair_price_band,
        fair_price_band_provenance=(
            selected_focus_fair_price_band.provenance
            if selected_focus_fair_price_band
            else no_data_fair_price_band_provenance()
        ),
        physical_stems=focus_physical_stems,
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


@router.get("/table", response_model=ForwardCurveTableResponse, summary="Forward curve monitoring table")
async def get_forward_curve_table(
    windows: Optional[list[str]] = Query(None, description="Optional canonical windows. Repeat the query parameter to request multiple windows."),
    market_products: Optional[list[MarketProduct]] = Query(
        None,
        description="Optional canonical market products. Repeat to filter the matrix.",
    ),
    delivery_point_ids: Optional[list[UUID]] = Query(
        None,
        description="Optional approved delivery point UUIDs. Repeat to filter the matrix.",
    ),
    db: AsyncSession = Depends(get_db),
) -> ForwardCurveTableResponse:
    """Return a compact matrix; use ``/slice`` for bounded evidence detail."""

    try:
        return await forward_curve_market_slices.load_table(
            db,
            windows=windows,
            market_products=(
                [value.value for value in market_products]
                if market_products
                else None
            ),
            delivery_point_ids=delivery_point_ids,
        )
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@router.get("/slice", response_model=ForwardCurveSliceResponse, summary="Forward curve selected slice")
async def get_forward_curve_slice(
    market_product: MarketProduct = Query(..., description="Canonical market product"),
    delivery_point_id: UUID = Query(..., description="Approved delivery point UUID"),
    availability_window: str = Query(..., description="Canonical window: SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL"),
    db: AsyncSession = Depends(get_db),
) -> ForwardCurveSliceResponse:
    """Return bounded graph-ready evidence for one exact market slice."""

    try:
        return await forward_curve_market_slices.load_slice(
            db,
            market_product=market_product.value,
            delivery_point_id=delivery_point_id,
            availability_window=availability_window,
        )
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


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

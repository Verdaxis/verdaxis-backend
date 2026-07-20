"""Canonical Forward Curve market-slice read models."""
from __future__ import annotations

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from uuid import UUID

from sqlalchemy import case, func, or_, select, tuple_
from sqlalchemy.ext.asyncio import AsyncSession

from app.market_catalog import (
    DELIVERY_POINT_DISPLAY_ORDER,
    MARKET_PRODUCT_CODES,
    PRODUCTS_BY_CODE,
)
from app.models.benchmark import Benchmark
from app.models.catalog import DeliveryPoint, Product
from app.models.user import OrganizationProvenance
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide, Trade, TradeStatus
from app.schemas.curves import (
    ForwardCurveBoardDepthLevel,
    ForwardCurveBoardFairPriceBand,
    ForwardCurveBoardIndication,
    ForwardCurveBoardIndicationSummary,
    ForwardCurveBoardPhysicalStem,
    ForwardCurveBoardPhysicalStemSummary,
    ForwardCurveEvidenceLayer,
    ForwardCurveIndicationSide,
    ForwardCurveLabelPolicy,
    ForwardCurveLatestSignal,
    ForwardCurveMarketCell,
    ForwardCurvePhysicalStemStatus,
    ForwardCurveSliceResponse,
    ForwardCurveSliceEvidencePoint,
    ForwardCurveSliceTrade,
    ForwardCurveStalenessStatus,
    ForwardCurveTableColumn,
    ForwardCurveTableCell,
    ForwardCurveTableResponse,
    ForwardCurveTableRow,
    MarketSignalType,
    no_data_label_policy,
)
from app.schemas.market_activity import (
    MarketDemoStatus,
    MarketScope,
    MarketSourceKind,
)
from app.services.availability_windows import (
    CALENDAR_WINDOW_RE,
    MONTH_WINDOW_RE,
    QUARTER_WINDOW_RE,
    SPOT_WINDOW,
    availability_window_display_label,
    availability_window_sort_key,
    normalize_availability_window,
    tradable_availability_windows,
)
from app.services.market_provenance import (
    MarketEvidenceScope,
    evidence_policy_for_scope,
    order_evidence_clause,
    public_order_evidence_clause,
    public_trade_evidence_clause,
    select_aggregate_evidence,
    trade_evidence_clause,
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
)


ACTIVE_ORDER_STATUSES = [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
CONFIRMED_TRADE_STATUSES = [TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID]
TRADE_LOOKBACK_DAYS = 30
FRESH_DAYS = 7
MAX_TABLE_WINDOWS = 16
MAX_DEPTH_LEVELS = 10
MAX_SLICE_TRADES = 8
MAX_SLICE_INDICATIONS = 10
MAX_SLICE_STEMS = 6

APPROVED_MARKET_PRODUCTS = MARKET_PRODUCT_CODES
MARKET_PRODUCT_DISPLAY_NAMES = {
    code: PRODUCTS_BY_CODE[code].name for code in MARKET_PRODUCT_CODES
}
MARKET_PRODUCT_BY_DISPLAY_NAME = {
    display_name: market_product
    for market_product, display_name in MARKET_PRODUCT_DISPLAY_NAMES.items()
}
APPROVED_DELIVERY_POINT_NAMES = tuple(DELIVERY_POINT_DISPLAY_ORDER)


@dataclass(frozen=True)
class ProductGroup:
    market_product: str
    product_name: str
    representative_product_id: UUID
    product_ids: tuple[UUID, ...]


@dataclass(frozen=True)
class SliceKey:
    market_product: str
    delivery_point_id: UUID
    availability_window: str


@dataclass(frozen=True)
class ViewerContext:
    """Visibility context for public monitoring projections."""

    include_demo: bool = True
    public: bool = True


def _money(value: Decimal | int | float | None) -> Decimal | None:
    if value is None:
        return None
    return Decimal(str(value)).quantize(Decimal("0.01"))


def _max_money(left: Decimal | int | float | None, right: Decimal | int | float | None) -> Decimal | None:
    left_money = _money(left)
    right_money = _money(right)
    if left_money is None:
        return right_money
    if right_money is None:
        return left_money
    return max(left_money, right_money)


def _min_money(left: Decimal | int | float | None, right: Decimal | int | float | None) -> Decimal | None:
    left_money = _money(left)
    right_money = _money(right)
    if left_money is None:
        return right_money
    if right_money is None:
        return left_money
    return min(left_money, right_money)


def _aware_utc(value: datetime | None) -> datetime | None:
    if value is None:
        return None
    if value.tzinfo is None:
        return value.replace(tzinfo=timezone.utc)
    return value.astimezone(timezone.utc)


def _strict_normalize_window(value: str) -> str:
    normalized = normalize_availability_window(value)
    raw = value.strip()
    if raw != normalized and not (raw.upper() == SPOT_WINDOW and normalized == SPOT_WINDOW):
        raise ValueError("availability_window must be canonical: SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL")
    if (
        normalized == SPOT_WINDOW
        or MONTH_WINDOW_RE.fullmatch(normalized)
        or QUARTER_WINDOW_RE.fullmatch(normalized)
        or CALENDAR_WINDOW_RE.fullmatch(normalized)
    ):
        return normalized
    raise ValueError("availability_window must be canonical: SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL")


def normalize_public_windows(values: Sequence[str] | None, *, now: datetime | None = None) -> list[str]:
    windows = list(values) if values else default_curve_windows(now)
    normalized: list[str] = []
    seen: set[str] = set()
    for value in windows:
        window = _strict_normalize_window(value)
        if window in seen:
            continue
        seen.add(window)
        normalized.append(window)
    normalized.sort(key=availability_window_sort_key)
    if len(normalized) > MAX_TABLE_WINDOWS:
        raise ValueError(f"At most {MAX_TABLE_WINDOWS} windows may be requested")
    return normalized


def default_curve_windows(now: datetime | None = None) -> list[str]:
    current = now or datetime.now(timezone.utc)
    return tradable_availability_windows(today=current.date())


def _window_group(window: str) -> str:
    if window == SPOT_WINDOW:
        return "Spot"
    if MONTH_WINDOW_RE.fullmatch(window):
        return "Monthly"
    if QUARTER_WINDOW_RE.fullmatch(window):
        return "Quarterly"
    if CALENDAR_WINDOW_RE.fullmatch(window):
        return "Calendar"
    return "Other"


def _staleness(observed_at: datetime | None, *, now: datetime) -> ForwardCurveStalenessStatus:
    normalized_observed_at = _aware_utc(observed_at)
    normalized_now = _aware_utc(now) or datetime.now(timezone.utc)
    if normalized_observed_at is None:
        return ForwardCurveStalenessStatus.NO_DATA
    if normalized_observed_at >= normalized_now - timedelta(days=FRESH_DAYS):
        return ForwardCurveStalenessStatus.FRESH
    return ForwardCurveStalenessStatus.STALE


def _label_policy(
    *,
    label: str,
    tooltip: str,
    allowed_terms: list[str] | None = None,
    disclaimer: str | None = None,
) -> ForwardCurveLabelPolicy:
    return ForwardCurveLabelPolicy(
        public_label=label,
        tooltip=tooltip,
        allowed_terms=allowed_terms or [],
        forbidden_terms=["live", "top-of-book", "firm", "market price", "last", "clearing"],
        disclaimer=disclaimer,
    )


def _no_data_indication_summary() -> ForwardCurveBoardIndicationSummary:
    return no_data_summary_for_signal(MarketSignalType.MARKET_INDICATION)


def _no_data_stem_summary() -> ForwardCurveBoardPhysicalStemSummary:
    return no_data_summary_for_signal(MarketSignalType.PHYSICAL_STEM)


def source_kind_from_counts_for_depth(demo_status: MarketDemoStatus) -> MarketSourceKind:
    if demo_status == MarketDemoStatus.REAL_ONLY:
        return MarketSourceKind.LIVE_ORDER
    if demo_status == MarketDemoStatus.DEMO_ONLY:
        return MarketSourceKind.DEMO_SEED
    if demo_status == MarketDemoStatus.MIXED:
        return MarketSourceKind.MIXED_SOURCE
    if demo_status == MarketDemoStatus.NOT_APPLICABLE:
        return MarketSourceKind.NO_DATA
    return MarketSourceKind.UNKNOWN


class ForwardCurveMarketSliceService:
    """Build canonical, redacted Forward Curve public read models."""

    async def load_product_groups(self, db: AsyncSession) -> list[ProductGroup]:
        result = await db.execute(
            select(Product).where(canonical_product_clause(Product))
        )
        grouped: dict[str, list[Product]] = {key: [] for key in APPROVED_MARKET_PRODUCTS}
        for product in result.scalars().all():
            market_product = MARKET_PRODUCT_BY_DISPLAY_NAME.get(product.name)
            if market_product is not None:
                grouped[market_product].append(product)

        groups: list[ProductGroup] = []
        for market_product in APPROVED_MARKET_PRODUCTS:
            products = sorted(grouped.get(market_product, []), key=lambda item: (item.name, str(item.id)))
            if not products:
                continue
            groups.append(
                ProductGroup(
                    market_product=market_product,
                    product_name=MARKET_PRODUCT_DISPLAY_NAMES.get(market_product, products[0].name),
                    representative_product_id=products[0].id,
                    product_ids=tuple(product.id for product in products),
                )
            )
        return groups

    async def load_delivery_points(self, db: AsyncSession) -> list[DeliveryPoint]:
        display_order = case(
            dict(DELIVERY_POINT_DISPLAY_ORDER),
            value=DeliveryPoint.name,
            else_=999,
        )
        result = await db.execute(
            select(DeliveryPoint)
            .where(canonical_delivery_point_clause(DeliveryPoint))
            .order_by(display_order, DeliveryPoint.name)
        )
        return result.scalars().all()

    async def load_table(
        self,
        db: AsyncSession,
        *,
        windows: Sequence[str] | None = None,
        market_products: Sequence[str] | None = None,
        delivery_point_ids: Sequence[UUID] | None = None,
        viewer_context: ViewerContext | None = None,
    ) -> ForwardCurveTableResponse:
        context = viewer_context or ViewerContext()
        generated_at = datetime.now(timezone.utc)
        normalized_windows = normalize_public_windows(windows, now=generated_at)
        product_groups = await self.load_product_groups(db)
        delivery_points = await self.load_delivery_points(db)
        if market_products:
            requested_products = {
                value.value if hasattr(value, "value") else str(value)
                for value in market_products
            }
            invalid_products = requested_products.difference(APPROVED_MARKET_PRODUCTS)
            if invalid_products:
                raise ValueError("market_products must contain canonical product identities")
            product_groups = [
                group
                for group in product_groups
                if group.market_product in requested_products
            ]
            if {group.market_product for group in product_groups} != requested_products:
                raise ValueError("a requested market product is not active")
        if delivery_point_ids:
            requested_points = {UUID(str(value)) for value in delivery_point_ids}
            delivery_points = [
                point for point in delivery_points if point.id in requested_points
            ]
            if {point.id for point in delivery_points} != requested_points:
                raise ValueError("a requested delivery point is not approved or active")
        keys = [
            SliceKey(group.market_product, point.id, window)
            for group in product_groups
            for point in delivery_points
            for window in normalized_windows
        ]
        cells = await self.load_many(
            db,
            keys,
            product_groups=product_groups,
            delivery_points=delivery_points,
            viewer_context=context,
            generated_at=generated_at,
        )

        rows: list[ForwardCurveTableRow] = []
        for group in product_groups:
            for point in delivery_points:
                row_cells = {
                    window: ForwardCurveTableCell.model_validate(
                        cells[
                            SliceKey(group.market_product, point.id, window)
                        ].model_dump()
                    )
                    for window in normalized_windows
                }
                rows.append(
                    ForwardCurveTableRow(
                        row_key=f"{group.market_product}:{point.id}",
                        market_product=group.market_product,
                        product_name=group.product_name,
                        representative_product_id=group.representative_product_id,
                        product_count=len(group.product_ids),
                        delivery_point_id=point.id,
                        delivery_point_name=point.name,
                        region=point.region,
                        cells=row_cells,
                    )
                )

        latest_signals = self._latest_signals(cells.values())
        return ForwardCurveTableResponse(
            columns=[
                ForwardCurveTableColumn(
                    availability_window=window,
                    display_label=availability_window_display_label(window),
                    group=_window_group(window),
                )
                for window in normalized_windows
            ],
            rows=rows,
            latest_signals=latest_signals,
            generated_at=generated_at,
        )

    async def load_slice(
        self,
        db: AsyncSession,
        *,
        market_product: str,
        delivery_point_id: UUID,
        availability_window: str,
        viewer_context: ViewerContext | None = None,
    ) -> ForwardCurveSliceResponse:
        context = viewer_context or ViewerContext()
        generated_at = datetime.now(timezone.utc)
        if market_product not in APPROVED_MARKET_PRODUCTS:
            raise ValueError("market_product is not an approved canonical product")
        window = _strict_normalize_window(availability_window)
        product_groups = await self.load_product_groups(db)
        delivery_points = await self.load_delivery_points(db)
        group = next((item for item in product_groups if item.market_product == market_product), None)
        point = next((item for item in delivery_points if item.id == delivery_point_id), None)
        if group is None:
            raise ValueError("market_product is not active")
        if point is None:
            raise ValueError("delivery_point_id is not approved or active")

        key = SliceKey(market_product, delivery_point_id, window)
        cell = (
            await self.load_many(
                db,
                [key],
                product_groups=[group],
                delivery_points=[point],
                viewer_context=context,
                generated_at=generated_at,
            )
        )[key]
        depth_bids, depth_asks = await self._load_depth(db, group=group, point=point, window=window)
        trades = await self._load_slice_trades(db, group=group, point=point, window=window)
        indications = await load_latest_indications_for_focus(
            db,
            market_product=market_product,
            delivery_point_id=delivery_point_id,
            availability_window=window,
            limit=MAX_SLICE_INDICATIONS,
        )
        stems = (
            await load_physical_stems_for_focus(
                db,
                market_product=market_product,
                delivery_point_id=delivery_point_id,
                availability_window=window,
                limit=MAX_SLICE_STEMS,
            )
        )[:MAX_SLICE_STEMS]
        windows = normalize_public_windows(None, now=generated_at)
        previous_window = None
        next_window = None
        if window in windows:
            index = windows.index(window)
            previous_window = windows[index - 1] if index > 0 else None
            next_window = windows[index + 1] if index < len(windows) - 1 else None

        return ForwardCurveSliceResponse(
            cell=cell,
            previous_window=previous_window,
            next_window=next_window,
            depth_bids=depth_bids,
            depth_asks=depth_asks,
            trades=trades,
            indications=indications,
            fair_price_band=cell.fair_price_band,
            physical_stems=stems,
            evidence_points=self._evidence_points(cell, depth_bids, depth_asks, trades, indications, stems),
            generated_at=generated_at,
        )

    async def load_many(
        self,
        db: AsyncSession,
        keys: Iterable[SliceKey],
        *,
        product_groups: Sequence[ProductGroup] | None = None,
        delivery_points: Sequence[DeliveryPoint] | None = None,
        viewer_context: ViewerContext | None = None,
        generated_at: datetime | None = None,
    ) -> dict[SliceKey, ForwardCurveMarketCell]:
        context = viewer_context or ViewerContext()
        now = generated_at or datetime.now(timezone.utc)
        normalized_keys = self._normalize_keys(keys)
        if not normalized_keys:
            return {}

        product_groups = list(product_groups or await self.load_product_groups(db))
        delivery_points = list(delivery_points or await self.load_delivery_points(db))
        groups_by_product = {group.market_product: group for group in product_groups}
        points_by_id = {point.id: point for point in delivery_points}
        allowed_point_ids = set(points_by_id)
        allowed_keys = [
            key
            for key in normalized_keys
            if key.market_product in groups_by_product and key.delivery_point_id in allowed_point_ids
        ]
        if not allowed_keys:
            return {}

        orderbook = await self._load_orderbook(db, allowed_keys, groups_by_product, context)
        trade_prints = await self._load_latest_trades(db, allowed_keys, groups_by_product)
        benchmarks = await self._load_persisted_benchmarks(db, allowed_keys)
        signal_keys: list[SignalKey] = [(key.market_product, key.delivery_point_id, key.availability_window) for key in allowed_keys]
        indication_summaries = await load_indication_summaries(db, signal_keys)
        physical_summaries = await load_physical_stem_summaries(db, signal_keys)
        fair_bands = await load_fair_price_bands(db, signal_keys)

        cells: dict[SliceKey, ForwardCurveMarketCell] = {}
        for key in allowed_keys:
            group = groups_by_product[key.market_product]
            point = points_by_id[key.delivery_point_id]
            order_bucket = orderbook.get(key, {})
            trade = trade_prints.get(key)
            benchmark = benchmarks.get(key)
            indication = indication_summaries.get(
                (key.market_product, key.delivery_point_id, key.availability_window),
                _no_data_indication_summary(),
            )
            physical = physical_summaries.get(
                (key.market_product, key.delivery_point_id, key.availability_window),
                _no_data_stem_summary(),
            )
            fair_band = fair_bands.get((key.market_product, key.delivery_point_id, key.availability_window))
            cells[key] = self._build_cell(
                key=key,
                group=group,
                point=point,
                order_bucket=order_bucket,
                trade=trade,
                benchmark=benchmark,
                indication_summary=indication,
                fair_price_band=fair_band,
                physical_stem_summary=physical,
                generated_at=now,
            )
        return cells

    def _normalize_keys(self, keys: Iterable[SliceKey]) -> list[SliceKey]:
        normalized: list[SliceKey] = []
        seen: set[SliceKey] = set()
        for key in keys:
            normalized_key = SliceKey(
                market_product=str(key.market_product),
                delivery_point_id=key.delivery_point_id,
                availability_window=_strict_normalize_window(key.availability_window),
            )
            if normalized_key.market_product not in APPROVED_MARKET_PRODUCTS:
                raise ValueError("market_product is not an approved canonical product")
            if normalized_key in seen:
                continue
            seen.add(normalized_key)
            normalized.append(normalized_key)
        return normalized

    async def _load_orderbook(
        self,
        db: AsyncSession,
        keys: Sequence[SliceKey],
        groups_by_product: dict[str, ProductGroup],
        context: ViewerContext,
    ) -> dict[SliceKey, dict[str, object]]:
        product_id_to_market_product = {
            product_id: group.market_product
            for group in groups_by_product.values()
            for product_id in group.product_ids
        }
        product_ids = list(product_id_to_market_product)
        delivery_point_ids = sorted({key.delivery_point_id for key in keys}, key=str)
        windows = sorted({key.availability_window for key in keys})
        requested = set(keys)
        if not product_ids or not delivery_point_ids or not windows:
            return {}

        demo_clause = order_evidence_clause(
            OrderBookOrder.provenance,
            MarketEvidenceScope.DEMO,
        )
        real_clause = order_evidence_clause(
            OrderBookOrder.provenance,
            MarketEvidenceScope.REAL,
        )
        unknown_clause = OrderBookOrder.provenance == OrganizationProvenance.UNKNOWN.value
        observed_at = func.coalesce(OrderBookOrder.updated_at, OrderBookOrder.created_at)
        stmt = (
            select(
                OrderBookOrder.product_id,
                OrderBookOrder.delivery_point_id,
                OrderBookOrder.availability_window,
                OrderBookOrder.side,
                func.max(case((real_clause, OrderBookOrder.price_per_mt_usd))).label("real_max_price"),
                func.min(case((real_clause, OrderBookOrder.price_per_mt_usd))).label("real_min_price"),
                func.max(case((demo_clause, OrderBookOrder.price_per_mt_usd))).label("demo_max_price"),
                func.min(case((demo_clause, OrderBookOrder.price_per_mt_usd))).label("demo_min_price"),
                func.sum(case((real_clause, OrderBookOrder.remaining_quantity_mt), else_=0)).label("real_volume_mt"),
                func.sum(case((demo_clause, OrderBookOrder.remaining_quantity_mt), else_=0)).label("demo_volume_mt"),
                func.sum(case((real_clause, 1), else_=0)).label("real_order_count"),
                func.sum(case((demo_clause, 1), else_=0)).label("demo_order_count"),
                func.sum(case((unknown_clause, 1), else_=0)).label("unknown_order_count"),
                func.max(case((real_clause, observed_at))).label("real_last_order_at"),
                func.max(case((demo_clause, observed_at))).label("demo_last_order_at"),
            )
            .where(
                OrderBookOrder.status.in_(ACTIVE_ORDER_STATUSES),
                current_public_order_clause(OrderBookOrder),
                OrderBookOrder.product_id.in_(product_ids),
                OrderBookOrder.delivery_point_id.in_(delivery_point_ids),
                OrderBookOrder.availability_window.in_(windows),
                (
                    real_clause
                    if not context.include_demo
                    else or_(
                        public_order_evidence_clause(OrderBookOrder.provenance),
                        unknown_clause,
                    )
                ),
            )
            .group_by(
                OrderBookOrder.product_id,
                OrderBookOrder.delivery_point_id,
                OrderBookOrder.availability_window,
                OrderBookOrder.side,
            )
        )
        result = await db.execute(stmt)
        buckets: dict[SliceKey, dict[str, object]] = {}
        for row in result.all():
            market_product = product_id_to_market_product.get(row.product_id)
            if market_product is None or row.delivery_point_id is None:
                continue
            key = SliceKey(market_product, row.delivery_point_id, normalize_availability_window(row.availability_window))
            if key not in requested:
                continue
            bucket = buckets.setdefault(
                key,
                {"real_volume_mt": Decimal("0"), "demo_volume_mt": Decimal("0")},
            )
            side = row.side.value if hasattr(row.side, "value") else str(row.side)
            if side == OrderSide.BID.value:
                bucket["real_best_bid"] = _max_money(bucket.get("real_best_bid"), row.real_max_price)
                bucket["demo_best_bid"] = _max_money(bucket.get("demo_best_bid"), row.demo_max_price)
            elif side == OrderSide.ASK.value:
                bucket["real_best_ask"] = _min_money(bucket.get("real_best_ask"), row.real_min_price)
                bucket["demo_best_ask"] = _min_money(bucket.get("demo_best_ask"), row.demo_min_price)
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

    async def _load_latest_trades(
        self,
        db: AsyncSession,
        keys: Sequence[SliceKey],
        groups_by_product: dict[str, ProductGroup],
    ) -> dict[SliceKey, dict[str, object]]:
        del groups_by_product  # Historical identity comes only from trade snapshots.
        market_products = sorted({key.market_product for key in keys})
        delivery_point_ids = sorted({key.delivery_point_id for key in keys}, key=str)
        windows = sorted({key.availability_window for key in keys})
        requested = set(keys)
        if not market_products or not delivery_point_ids or not windows:
            return {}

        cutoff = datetime.now(timezone.utc) - timedelta(days=TRADE_LOOKBACK_DAYS)
        stmt = (
            select(
                Trade.price_per_mt_usd,
                Trade.quantity_mt,
                Trade.confirmed_at,
                Trade.buyer_id,
                Trade.seller_id,
                Trade.product_id,
                Trade.market_product,
                Trade.delivery_point_id,
                Trade.availability_window,
                Trade.buyer_provenance,
                Trade.seller_provenance,
            )
            .where(
                Trade.status.in_(CONFIRMED_TRADE_STATUSES),
                Trade.confirmed_at.isnot(None),
                Trade.confirmed_at >= cutoff,
                Trade.market_product.in_(market_products),
                Trade.delivery_point_id.in_(delivery_point_ids),
                Trade.availability_window.in_(windows),
                public_trade_evidence_clause(
                    Trade,
                    confirmed_since=cutoff,
                ),
            )
            .order_by(Trade.confirmed_at.desc(), Trade.id.desc())
        )
        result = await db.execute(stmt)
        candidates: dict[SliceKey, dict[MarketEvidenceScope, dict[str, object]]] = {}
        for row in result.all():
            if row.market_product is None or row.delivery_point_id is None:
                continue
            key = SliceKey(
                row.market_product,
                row.delivery_point_id,
                normalize_availability_window(row.availability_window),
            )
            if key not in requested:
                continue
            scope = (
                MarketEvidenceScope.REAL
                if row.buyer_provenance == OrganizationProvenance.REAL
                or row.buyer_provenance == OrganizationProvenance.REAL.value
                else MarketEvidenceScope.DEMO
            )
            scoped = candidates.setdefault(key, {})
            if scope in scoped:
                continue
            policy = evidence_policy_for_scope(
                scope,
                real_source=MarketSourceKind.CONFIRMED_TRADE,
            )
            scoped[scope] = {
                "price_per_mt_usd": row.price_per_mt_usd,
                "quantity_mt": row.quantity_mt,
                "confirmed_at": row.confirmed_at,
                "demo_status": policy.demo_status,
                "source_kind": policy.source_kind,
            }
        return {
            key: scoped.get(MarketEvidenceScope.REAL)
            or scoped[MarketEvidenceScope.DEMO]
            for key, scoped in candidates.items()
        }

    async def _load_persisted_benchmarks(
        self,
        db: AsyncSession,
        keys: Sequence[SliceKey],
    ) -> dict[SliceKey, Benchmark]:
        key_tuples = [(key.market_product, key.delivery_point_id, key.availability_window) for key in keys]
        if not key_tuples:
            return {}
        stmt = select(Benchmark).where(
            tuple_(Benchmark.market_product, Benchmark.delivery_point_id, Benchmark.availability_window).in_(key_tuples)
        )
        result = await db.execute(stmt)
        return {
            SliceKey(row.market_product, row.delivery_point_id, row.availability_window): row
            for row in result.scalars().all()
        }

    def _build_cell(
        self,
        *,
        key: SliceKey,
        group: ProductGroup,
        point: DeliveryPoint,
        order_bucket: dict[str, object],
        trade: dict[str, object] | None,
        benchmark: Benchmark | None,
        indication_summary: ForwardCurveBoardIndicationSummary,
        fair_price_band: ForwardCurveBoardFairPriceBand | None,
        physical_stem_summary: ForwardCurveBoardPhysicalStemSummary,
        generated_at: datetime,
    ) -> ForwardCurveMarketCell:
        real_order_count = int(order_bucket.get("real_order_count") or 0)
        demo_order_count = int(order_bucket.get("demo_order_count") or 0)
        unknown_order_count = int(order_bucket.get("unknown_order_count") or 0)
        selection = select_aggregate_evidence(
            real_count=real_order_count,
            demo_count=demo_order_count,
            unknown_count=unknown_order_count,
            real_source=MarketSourceKind.LIVE_ORDER,
        )
        selected_scope = selection.scope
        prefix = selection.value_prefix
        real_best_bid = _money(order_bucket.get("real_best_bid"))
        real_best_ask = _money(order_bucket.get("real_best_ask"))
        demo_best_bid = _money(order_bucket.get("demo_best_bid"))
        demo_best_ask = _money(order_bucket.get("demo_best_ask"))
        best_bid = real_best_bid if selected_scope == MarketEvidenceScope.REAL else demo_best_bid
        best_ask = real_best_ask if selected_scope == MarketEvidenceScope.REAL else demo_best_ask
        spread = _money(best_ask - best_bid) if best_bid is not None and best_ask is not None else None
        volume_mt = (
            _money(order_bucket.get(f"{prefix}_volume_mt")) or Decimal("0")
            if prefix is not None
            else Decimal("0")
        )
        order_count = (
            int(order_bucket.get(f"{prefix}_order_count") or 0)
            if prefix is not None
            else 0
        )
        demo_status = selection.demo_status
        order_observed_at = (
            order_bucket.get(f"{prefix}_last_order_at")
            if prefix is not None
            else None
        )
        benchmark_mid = _money(benchmark.price_per_mt_usd) if benchmark else None
        benchmark_observed_at = (benchmark.updated_at or benchmark.created_at) if benchmark else None
        primary_value, primary_signal, primary_source, label, policy, observed_at, is_executable, is_reference = (
            self._primary_mark(
                trade=trade,
                real_best_bid=real_best_bid,
                real_best_ask=real_best_ask,
                demo_best_bid=demo_best_bid,
                demo_best_ask=demo_best_ask,
                demo_status=demo_status,
                order_observed_at=order_observed_at,
                indication_summary=indication_summary,
                fair_price_band=fair_price_band,
                benchmark_mid=benchmark_mid,
                benchmark_observed_at=benchmark_observed_at,
            )
        )
        observed_at = _aware_utc(observed_at)
        benchmark_observed_at = _aware_utc(benchmark_observed_at)

        return ForwardCurveMarketCell(
            market_product=key.market_product,
            product_name=group.product_name,
            representative_product_id=group.representative_product_id,
            product_count=len(group.product_ids),
            delivery_point_id=point.id,
            delivery_point_name=point.name,
            region=point.region,
            availability_window=key.availability_window,
            primary_value=primary_value,
            primary_signal_type=primary_signal,
            primary_source_kind=primary_source,
            public_source_label=label,
            label_policy=policy,
            staleness_status=_staleness(observed_at, now=generated_at),
            is_executable=is_executable,
            is_reference=is_reference,
            demo_status=trade.get("demo_status") if trade else demo_status,
            scope=MarketScope.DELIVERY_POINT,
            observed_at=observed_at,
            generated_at=generated_at,
            best_bid=best_bid,
            best_ask=best_ask,
            spread=spread,
            volume_mt=volume_mt,
            order_count=order_count,
            real_order_count=real_order_count,
            demo_order_count=demo_order_count,
            unknown_order_count=unknown_order_count,
            real_best_bid=real_best_bid,
            real_best_ask=real_best_ask,
            demo_best_bid=demo_best_bid,
            demo_best_ask=demo_best_ask,
            benchmark_mid=benchmark_mid,
            benchmark_source_kind=MarketSourceKind.BENCHMARK_REFERENCE if benchmark else MarketSourceKind.NO_DATA,
            benchmark_observed_at=benchmark_observed_at,
            indication_summary=indication_summary,
            fair_price_band=fair_price_band,
            fair_price_band_provenance=(
                fair_price_band.provenance if fair_price_band else no_data_fair_price_band_provenance()
            ),
            physical_stem_summary=physical_stem_summary,
        )

    def _primary_mark(
        self,
        *,
        trade: dict[str, object] | None,
        real_best_bid: Decimal | None,
        real_best_ask: Decimal | None,
        demo_best_bid: Decimal | None,
        demo_best_ask: Decimal | None,
        demo_status: MarketDemoStatus,
        order_observed_at,
        indication_summary: ForwardCurveBoardIndicationSummary,
        fair_price_band: ForwardCurveBoardFairPriceBand | None,
        benchmark_mid: Decimal | None,
        benchmark_observed_at,
    ) -> tuple[
        Decimal | None,
        MarketSignalType,
        MarketSourceKind,
        str,
        ForwardCurveLabelPolicy,
        datetime | None,
        bool,
        bool,
    ]:
        if trade is not None:
            label = "Historical confirmed trade"
            return (
                _money(trade.get("price_per_mt_usd")),
                MarketSignalType.CONFIRMED_TRADE,
                trade.get("source_kind") or MarketSourceKind.CONFIRMED_TRADE,
                label,
                _label_policy(
                    label=label,
                    tooltip="Latest anonymized confirmed trade print for this exact product, port, and window.",
                    allowed_terms=["historical", "confirmed trade"],
                ),
                trade.get("confirmed_at"),
                False,
                False,
            )

        has_real_two_sided = real_best_bid is not None and real_best_ask is not None
        has_demo_two_sided = demo_best_bid is not None and demo_best_ask is not None
        if has_real_two_sided:
            midpoint = _money((real_best_bid + real_best_ask) / Decimal("2"))
            source_kind = MarketSourceKind.MIXED_SOURCE if demo_status == MarketDemoStatus.MIXED else MarketSourceKind.LIVE_ORDER
            label = "Executable orderbook midpoint"
            return (
                midpoint,
                MarketSignalType.ORDERBOOK_BID,
                source_kind,
                label,
                _label_policy(
                    label=label,
                    tooltip="Midpoint of the best visible bid and ask for this exact slice. One-sided books do not create a midpoint.",
                    allowed_terms=["orderbook", "midpoint", "executable"] if has_real_two_sided else ["demo", "orderbook", "midpoint"],
                ),
                order_observed_at,
                True,
                False,
            )
        if demo_status == MarketDemoStatus.DEMO_ONLY and has_demo_two_sided:
            label = "Demo orderbook midpoint"
            return (
                _money((demo_best_bid + demo_best_ask) / Decimal("2")),
                MarketSignalType.ORDERBOOK_BID,
                MarketSourceKind.DEMO_SEED,
                label,
                _label_policy(
                    label=label,
                    tooltip="Demo midpoint of the best visible bid and ask for this exact slice. Not executable user liquidity.",
                    allowed_terms=["demo", "orderbook", "midpoint"],
                ),
                order_observed_at,
                False,
                False,
            )

        indication_mid = _money(indication_summary.latest_mid_price_per_mt_usd)
        if indication_mid is None and indication_summary.latest_bid_price_per_mt_usd is not None and indication_summary.latest_ask_price_per_mt_usd is not None:
            indication_mid = _money(
                (indication_summary.latest_bid_price_per_mt_usd + indication_summary.latest_ask_price_per_mt_usd)
                / Decimal("2")
            )
        if indication_mid is not None:
            label = "Market indication"
            return (
                indication_mid,
                MarketSignalType.MARKET_INDICATION,
                MarketSourceKind.DEMO_SEED
                if indication_summary.provenance.demo_status == MarketDemoStatus.DEMO_ONLY
                else MarketSourceKind.UNKNOWN,
                label,
                _label_policy(
                    label=label,
                    tooltip="Non-executable market indication for this exact slice.",
                    allowed_terms=["indication", "non-executable"],
                ),
                indication_summary.provenance.observed_at,
                False,
                True,
            )

        if (
            fair_price_band is not None
            and fair_price_band.mid_price_per_mt_usd is not None
        ):
            label = "Verdaxis fair band"
            return (
                _money(fair_price_band.mid_price_per_mt_usd),
                MarketSignalType.FAIR_PRICE_BAND,
                MarketSourceKind.BENCHMARK_REFERENCE,
                label,
                _label_policy(
                    label=label,
                    tooltip="Modelled fair-value band for this exact slice.",
                    allowed_terms=["fair band", "modelled"],
                    disclaimer="Indicative estimate only. Not legal, regulatory, tax, or compliance filing advice.",
                ),
                fair_price_band.provenance.observed_at,
                False,
                True,
            )

        if benchmark_mid is not None:
            label = "Benchmark reference"
            return (
                benchmark_mid,
                MarketSignalType.BENCHMARK_MID,
                MarketSourceKind.BENCHMARK_REFERENCE,
                label,
                _label_policy(
                    label=label,
                    tooltip="Persisted benchmark reference for this exact slice.",
                    allowed_terms=["benchmark", "reference"],
                ),
                benchmark_observed_at,
                False,
                True,
            )

        return (
            None,
            MarketSignalType.NO_DATA,
            MarketSourceKind.NO_DATA,
            "No data",
            no_data_label_policy(),
            None,
            False,
            False,
        )

    async def _load_depth(
        self,
        db: AsyncSession,
        *,
        group: ProductGroup,
        point: DeliveryPoint,
        window: str,
    ) -> tuple[list[ForwardCurveBoardDepthLevel], list[ForwardCurveBoardDepthLevel]]:
        demo_clause = order_evidence_clause(
            OrderBookOrder.provenance,
            MarketEvidenceScope.DEMO,
        )
        real_clause = order_evidence_clause(
            OrderBookOrder.provenance,
            MarketEvidenceScope.REAL,
        )
        stmt = (
            select(
                OrderBookOrder.side,
                OrderBookOrder.provenance,
                OrderBookOrder.price_per_mt_usd,
                func.sum(OrderBookOrder.remaining_quantity_mt).label("quantity_mt"),
                func.count(OrderBookOrder.id).label("order_count"),
                func.sum(case((real_clause, 1), else_=0)).label("real_order_count"),
                func.sum(case((demo_clause, 1), else_=0)).label("demo_order_count"),
            )
            .where(
                OrderBookOrder.status.in_(ACTIVE_ORDER_STATUSES),
                current_public_order_clause(OrderBookOrder),
                OrderBookOrder.product_id.in_(list(group.product_ids)),
                OrderBookOrder.delivery_point_id == point.id,
                OrderBookOrder.availability_window == window,
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
        return bids[:MAX_DEPTH_LEVELS], asks[:MAX_DEPTH_LEVELS]

    async def _load_slice_trades(
        self,
        db: AsyncSession,
        *,
        group: ProductGroup,
        point: DeliveryPoint,
        window: str,
    ) -> list[ForwardCurveSliceTrade]:
        cutoff = datetime.now(timezone.utc) - timedelta(days=TRADE_LOOKBACK_DAYS)
        for scope in (MarketEvidenceScope.REAL, MarketEvidenceScope.DEMO):
            policy = evidence_policy_for_scope(
                scope,
                real_source=MarketSourceKind.CONFIRMED_TRADE,
            )
            stmt = (
                select(Trade)
                .where(
                    Trade.status.in_(CONFIRMED_TRADE_STATUSES),
                    Trade.confirmed_at.isnot(None),
                    Trade.confirmed_at >= cutoff,
                    Trade.market_product == group.market_product,
                    Trade.delivery_point_id == point.id,
                    Trade.availability_window == window,
                    trade_evidence_clause(
                        Trade,
                        scope,
                        confirmed_since=cutoff,
                    ),
                )
                .order_by(Trade.confirmed_at.desc(), Trade.id.desc())
                .limit(MAX_SLICE_TRADES)
            )
            result = await db.execute(stmt)
            trades = [
                ForwardCurveSliceTrade(
                    price_per_mt_usd=_money(trade.price_per_mt_usd) or Decimal("0"),
                    quantity_mt=_money(trade.quantity_mt) or Decimal("0"),
                    confirmed_at=trade.confirmed_at,
                    source_kind=policy.source_kind,
                    demo_status=policy.demo_status,
                )
                for trade in result.scalars().all()
            ]
            if trades:
                return trades
        return []

    def _latest_signals(self, cells: Iterable[ForwardCurveMarketCell]) -> list[ForwardCurveLatestSignal]:
        populated = [cell for cell in cells if cell.primary_value is not None and cell.observed_at is not None]
        populated.sort(key=lambda cell: cell.observed_at or datetime.min.replace(tzinfo=timezone.utc), reverse=True)
        return [
            ForwardCurveLatestSignal(
                market_product=cell.market_product,
                delivery_point_id=cell.delivery_point_id,
                delivery_point_name=cell.delivery_point_name,
                availability_window=cell.availability_window,
                primary_value=cell.primary_value,
                primary_signal_type=cell.primary_signal_type,
                primary_source_kind=cell.primary_source_kind,
                public_source_label=cell.public_source_label,
                demo_status=cell.demo_status,
                observed_at=cell.observed_at,
                staleness_status=cell.staleness_status,
            )
            for cell in populated[:8]
        ]

    def _evidence_points(
        self,
        cell: ForwardCurveMarketCell,
        depth_bids: Sequence[ForwardCurveBoardDepthLevel],
        depth_asks: Sequence[ForwardCurveBoardDepthLevel],
        trades: Sequence[ForwardCurveSliceTrade],
        indications: Sequence[ForwardCurveBoardIndication],
        stems: Sequence[ForwardCurveBoardPhysicalStem],
    ) -> list[ForwardCurveSliceEvidencePoint]:
        points: list[ForwardCurveSliceEvidencePoint] = []
        points.extend(
            ForwardCurveSliceEvidencePoint(
                layer=ForwardCurveEvidenceLayer.HISTORICAL_TRADE,
                price_per_mt_usd=trade.price_per_mt_usd,
                quantity_mt=trade.quantity_mt,
                observed_at=trade.confirmed_at,
                public_source_label="Historical confirmed trade",
                source_kind=trade.source_kind,
                demo_status=trade.demo_status,
            )
            for trade in trades
        )
        points.extend(
            ForwardCurveSliceEvidencePoint(
                layer=ForwardCurveEvidenceLayer.ORDERBOOK_BID,
                side=ForwardCurveIndicationSide.BID,
                price_per_mt_usd=level.price_per_mt_usd,
                quantity_mt=level.quantity_mt,
                observed_at=cell.observed_at,
                public_source_label="Visible bid depth",
                source_kind=level.source_kind,
                demo_status=level.demo_status,
            )
            for level in depth_bids[:MAX_DEPTH_LEVELS]
        )
        points.extend(
            ForwardCurveSliceEvidencePoint(
                layer=ForwardCurveEvidenceLayer.ORDERBOOK_ASK,
                side=ForwardCurveIndicationSide.ASK,
                price_per_mt_usd=level.price_per_mt_usd,
                quantity_mt=level.quantity_mt,
                observed_at=cell.observed_at,
                public_source_label="Visible ask depth",
                source_kind=level.source_kind,
                demo_status=level.demo_status,
            )
            for level in depth_asks[:MAX_DEPTH_LEVELS]
        )
        for indication in indications:
            points.append(
                ForwardCurveSliceEvidencePoint(
                    layer=ForwardCurveEvidenceLayer.MARKET_INDICATION,
                    side=indication.side,
                    price_per_mt_usd=indication.price_per_mt_usd,
                    quantity_mt=indication.quantity_mt,
                    observed_at=indication.provenance.observed_at,
                    public_source_label="Market indication",
                    source_kind=MarketSourceKind.DEMO_SEED
                    if indication.provenance.demo_status == MarketDemoStatus.DEMO_ONLY
                    else MarketSourceKind.UNKNOWN,
                    demo_status=indication.provenance.demo_status,
                )
            )
        if (
            cell.fair_price_band is not None
            and cell.fair_price_band.mid_price_per_mt_usd is not None
        ):
            points.append(
                ForwardCurveSliceEvidencePoint(
                    layer=ForwardCurveEvidenceLayer.FAIR_PRICE_BAND,
                    price_per_mt_usd=cell.fair_price_band.mid_price_per_mt_usd,
                    low_price_per_mt_usd=cell.fair_price_band.low_price_per_mt_usd,
                    high_price_per_mt_usd=cell.fair_price_band.high_price_per_mt_usd,
                    observed_at=cell.fair_price_band.provenance.observed_at,
                    public_source_label="Verdaxis fair band",
                    source_kind=MarketSourceKind.BENCHMARK_REFERENCE,
                    demo_status=cell.fair_price_band.provenance.demo_status,
                )
            )
        if cell.benchmark_mid is not None:
            points.append(
                ForwardCurveSliceEvidencePoint(
                    layer=ForwardCurveEvidenceLayer.BENCHMARK_MID,
                    price_per_mt_usd=cell.benchmark_mid,
                    observed_at=cell.benchmark_observed_at,
                    public_source_label="Benchmark reference",
                    source_kind=cell.benchmark_source_kind,
                    demo_status=MarketDemoStatus.NOT_APPLICABLE,
                )
            )
        points.extend(
            ForwardCurveSliceEvidencePoint(
                layer=ForwardCurveEvidenceLayer.PHYSICAL_STEM,
                quantity_mt=stem.quantity_mt,
                observed_at=stem.provenance.observed_at,
                public_source_label=(
                    "Available physical stem"
                    if stem.status == ForwardCurvePhysicalStemStatus.AVAILABLE
                    else "Physical stem signal"
                ),
                source_kind=MarketSourceKind.DEMO_SEED
                if stem.provenance.demo_status == MarketDemoStatus.DEMO_ONLY
                else MarketSourceKind.UNKNOWN,
                demo_status=stem.provenance.demo_status,
            )
            for stem in stems[:MAX_SLICE_STEMS]
        )
        return points


forward_curve_market_slices = ForwardCurveMarketSliceService()

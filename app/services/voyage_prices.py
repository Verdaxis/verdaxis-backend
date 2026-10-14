"""Read-only market price resolution for the voyage fuel planner and market tools.

This module reads the database and never writes. Every price it returns states
its source and whether it is executable, and no result carries an
organization, user or order identifier. Design:
docs/plans/2026-10-14-voyage-mcp-design.md, "Prices".
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import date, datetime, timedelta
from decimal import ROUND_HALF_UP, Decimal
from typing import Sequence
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.exc import SQLAlchemyError
from sqlalchemy.ext.asyncio import AsyncSession

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCTS_BY_CODE
from app.models.orderbook import OrderBookOrder, OrderSide
from app.models.user import OrganizationProvenance
from app.schemas.market_activity import MarketDemoStatus
from app.schemas.voyage import (
    MarketSliceSnapshot,
    PriceLookup,
    PricePoint,
    PriceSource,
)
from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window
from app.services.benchmarks import get_benchmark_quotes
from app.services.public_order_scope import public_order_scope

# A departure inside this many days of today may be served from spot liquidity.
SPOT_LEAD_DAYS = 30
MONEY_QUANTUM = Decimal("0.01")
UNAVAILABLE_MESSAGE = "Market data is temporarily unavailable"

_SLICE_EVIDENCE = {
    MarketDemoStatus.REAL_ONLY: "live orders from real organizations",
    MarketDemoStatus.DEMO_ONLY: "demo preview liquidity, which is never executable",
    MarketDemoStatus.MIXED: (
        "a mix of live orders from real organizations and demo preview liquidity"
    ),
    MarketDemoStatus.NOT_APPLICABLE: "no open orders",
}


class MarketDataUnavailable(RuntimeError):
    """Raised when the market database cannot be read. Never leaks internals."""


@dataclass(frozen=True)
class _LookupTarget:
    """One requested price, with the catalog identity and windows it needs."""

    lookup: PriceLookup
    product_id: UUID
    delivery_point_id: UUID
    windows: tuple[str, ...]

    @property
    def month_window(self) -> str:
        return _month_window(self.lookup.departure_date)


def _month_window(day: date) -> str:
    return f"{day.year}-{day.month:02d}"


def _quarter_window(day: date) -> str:
    return f"{day.year}-Q{((day.month - 1) // 3) + 1}"


def candidate_windows(departure_date: date, *, today: date) -> tuple[str, ...]:
    """Return the availability windows to try for a departure, most specific first."""
    windows = [_month_window(departure_date), _quarter_window(departure_date)]
    if departure_date <= today + timedelta(days=SPOT_LEAD_DAYS):
        windows.insert(0, SPOT_WINDOW)
    return tuple(windows)


def _price_source(provenance) -> tuple[PriceSource, bool] | None:
    """Map an order's organization provenance to a price source, or None to skip it."""
    value = getattr(provenance, "value", provenance)
    if value == OrganizationProvenance.DEMO.value:
        return PriceSource.DEMO_ORDER, False
    if value == OrganizationProvenance.REAL.value:
        return PriceSource.LIVE_ORDER, True
    # TEST and UNKNOWN organizations never quote a price.
    return None


def _lookup_targets(lookups: Sequence[PriceLookup], *, today: date) -> list[_LookupTarget]:
    """Resolve catalog identities once, and skip anything the catalog does not know."""
    targets: list[_LookupTarget] = []
    for lookup in lookups:
        product = PRODUCTS_BY_CODE.get(lookup.fuel.value)
        delivery_point_id = DELIVERY_POINT_IDS.get(lookup.port)
        if product is None or delivery_point_id is None:
            continue
        targets.append(
            _LookupTarget(
                lookup=lookup,
                product_id=product.id,
                delivery_point_id=delivery_point_id,
                windows=candidate_windows(lookup.departure_date, today=today),
            )
        )
    return targets


async def _visible_ask_rows(
    db: AsyncSession,
    targets: Sequence[_LookupTarget],
    *,
    now: datetime,
) -> list:
    """Read every candidate ASK for every lookup in one query."""
    filters, joins = public_order_scope(side=OrderSide.ASK, as_of=now)
    filters.append(
        OrderBookOrder.product_id.in_({target.product_id for target in targets})
    )
    filters.append(
        OrderBookOrder.delivery_point_id.in_(
            {target.delivery_point_id for target in targets}
        )
    )
    filters.append(
        OrderBookOrder.availability_window.in_(
            {window for target in targets for window in target.windows}
        )
    )
    statement = select(
        OrderBookOrder.product_id,
        OrderBookOrder.delivery_point_id,
        OrderBookOrder.availability_window,
        OrderBookOrder.price_per_mt_usd,
        OrderBookOrder.acceptance_ordinal,
        OrderBookOrder.provenance,
        OrderBookOrder.carbon_intensity_gco2_mj,
        OrderBookOrder.energy_density_mj_kg,
    )
    for join_target, join_condition in joins:
        statement = statement.join(join_target, join_condition)
    return list((await db.execute(statement.where(*filters))).all())


def _price_point(target: _LookupTarget, row, window: str, *, now: datetime) -> PricePoint:
    source, executable = _price_source(row.provenance)
    return PricePoint(
        fuel=target.lookup.fuel,
        port=target.lookup.port,
        price_usd_per_mt=Decimal(str(row.price_per_mt_usd)).quantize(
            MONEY_QUANTUM, rounding=ROUND_HALF_UP
        ),
        source=source,
        executable=executable,
        availability_window=normalize_availability_window(window),
        observed_at=now,
        carbon_intensity_gco2e_per_mj=row.carbon_intensity_gco2_mj,
        energy_density_mj_per_kg=row.energy_density_mj_kg,
    )


def _resolve_from_asks(
    rows: Sequence,
    targets: Sequence[_LookupTarget],
    *,
    now: datetime,
) -> dict[PriceLookup, PricePoint]:
    """Pick, per lookup, the cheapest ask in the earliest window that has one."""
    by_slice: dict[tuple[UUID, UUID, str], list] = {}
    for row in rows:
        if _price_source(row.provenance) is None:
            continue
        key = (
            row.product_id,
            row.delivery_point_id,
            normalize_availability_window(row.availability_window),
        )
        by_slice.setdefault(key, []).append(row)

    prices: dict[PriceLookup, PricePoint] = {}
    for target in targets:
        for window in target.windows:
            candidates = by_slice.get(
                (target.product_id, target.delivery_point_id, window)
            )
            if not candidates:
                continue
            chosen = min(
                candidates,
                key=lambda row: (row.price_per_mt_usd, row.acceptance_ordinal),
            )
            prices[target.lookup] = _price_point(target, chosen, window, now=now)
            break
    return prices


def _apply_benchmarks(
    prices: dict[PriceLookup, PricePoint],
    targets: Sequence[_LookupTarget],
    quotes: dict,
    *,
    now: datetime,
) -> None:
    """Fill unresolved lookups from the indicative month-window benchmark."""
    for target in targets:
        if target.lookup in prices:
            continue
        quote = quotes.get(
            (target.lookup.fuel.value, target.delivery_point_id, target.month_window)
        )
        if quote is None:
            continue
        prices[target.lookup] = PricePoint(
            fuel=target.lookup.fuel,
            port=target.lookup.port,
            price_usd_per_mt=Decimal(str(quote.benchmark_price_per_mt_usd)).quantize(
                MONEY_QUANTUM, rounding=ROUND_HALF_UP
            ),
            source=PriceSource.BENCHMARK,
            executable=False,
            availability_window=quote.availability_window,
            observed_at=now,
        )


async def resolve_product_prices(
    db: AsyncSession,
    lookups: Sequence[PriceLookup],
    *,
    now: datetime,
) -> dict[PriceLookup, PricePoint]:
    """Resolve green-fuel prices from public asks, then from indicative benchmarks.

    A lookup with no visible ask and no benchmark is absent from the result, so
    the caller can name it as a missing input instead of inventing a price.
    """
    targets = _lookup_targets(lookups, today=now.date())
    if not targets:
        return {}

    try:
        rows = await _visible_ask_rows(db, targets, now=now)
        prices = _resolve_from_asks(rows, targets, now=now)
        unresolved = [target for target in targets if target.lookup not in prices]
        if unresolved:
            quotes = await get_benchmark_quotes(
                db,
                [
                    (
                        target.lookup.fuel.value,
                        target.delivery_point_id,
                        target.month_window,
                        target.lookup.port,
                    )
                    for target in unresolved
                ],
            )
            _apply_benchmarks(prices, unresolved, quotes, now=now)
    except SQLAlchemyError as error:
        raise MarketDataUnavailable(UNAVAILABLE_MESSAGE) from error
    return prices


def _demo_status(rows: Sequence) -> MarketDemoStatus:
    evidence = {
        getattr(row.provenance, "value", row.provenance) for row in rows
    }
    has_real = OrganizationProvenance.REAL.value in evidence
    has_demo = OrganizationProvenance.DEMO.value in evidence
    if has_real and has_demo:
        return MarketDemoStatus.MIXED
    if has_real:
        return MarketDemoStatus.REAL_ONLY
    if has_demo:
        return MarketDemoStatus.DEMO_ONLY
    return MarketDemoStatus.NOT_APPLICABLE


def _slice_note(demo_status: MarketDemoStatus, *, has_benchmark: bool) -> str:
    evidence = _SLICE_EVIDENCE[demo_status]
    benchmark = (
        "; the benchmark is an indicative reference, not an executable price"
        if has_benchmark
        else ""
    )
    return f"This slice shows {evidence}{benchmark}."


def _product_and_point_ids(
    market_product: str,
    delivery_point: str,
) -> tuple[UUID, UUID]:
    product = PRODUCTS_BY_CODE.get(market_product)
    if product is None:
        supported = ", ".join(sorted(PRODUCTS_BY_CODE))
        raise ValueError(
            f"Unsupported market product {market_product!r}. Supported products: {supported}"
        )
    delivery_point_id = DELIVERY_POINT_IDS.get(delivery_point)
    if delivery_point_id is None:
        supported = ", ".join(DELIVERY_POINT_IDS)
        raise ValueError(
            f"Unsupported delivery point {delivery_point!r}. Supported delivery points: {supported}"
        )
    return product.id, delivery_point_id


async def _slice_order_rows(
    db: AsyncSession,
    *,
    product_id: UUID,
    delivery_point_id: UUID,
    window: str,
    now: datetime,
) -> list:
    """Read both sides of one slice in a single query."""
    filters, joins = public_order_scope(as_of=now)
    filters.append(OrderBookOrder.product_id == product_id)
    filters.append(OrderBookOrder.delivery_point_id == delivery_point_id)
    filters.append(OrderBookOrder.availability_window == window)
    statement = select(
        OrderBookOrder.side,
        OrderBookOrder.price_per_mt_usd,
        OrderBookOrder.remaining_quantity_mt,
        OrderBookOrder.provenance,
    )
    for join_target, join_condition in joins:
        statement = statement.join(join_target, join_condition)
    return list((await db.execute(statement.where(*filters))).all())


async def market_slice_snapshot(
    db: AsyncSession,
    *,
    market_product: str,
    delivery_point: str,
    availability_window: str,
    now: datetime,
) -> MarketSliceSnapshot:
    """Describe one product/port/window slice without naming any participant."""
    product_id, delivery_point_id = _product_and_point_ids(market_product, delivery_point)
    window = normalize_availability_window(availability_window)

    try:
        rows = await _slice_order_rows(
            db,
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            window=window,
            now=now,
        )
        quotes = await get_benchmark_quotes(
            db, [(market_product, delivery_point_id, window, delivery_point)]
        )
    except SQLAlchemyError as error:
        raise MarketDataUnavailable(UNAVAILABLE_MESSAGE) from error

    bids = [row for row in rows if row.side == OrderSide.BID]
    asks = [row for row in rows if row.side == OrderSide.ASK]
    quote = quotes.get((market_product, delivery_point_id, window))
    benchmark_price = (
        Decimal(str(quote.benchmark_price_per_mt_usd)).quantize(
            MONEY_QUANTUM, rounding=ROUND_HALF_UP
        )
        if quote is not None
        else None
    )
    demo_status = _demo_status(rows)

    return MarketSliceSnapshot(
        market_product=market_product,
        delivery_point=delivery_point,
        availability_window=window,
        best_bid_usd_per_mt=(
            max(row.price_per_mt_usd for row in bids) if bids else None
        ),
        best_ask_usd_per_mt=(
            min(row.price_per_mt_usd for row in asks) if asks else None
        ),
        bid_order_count=len(bids),
        ask_order_count=len(asks),
        bid_quantity_mt=sum(
            (Decimal(str(row.remaining_quantity_mt)) for row in bids), Decimal("0")
        ),
        ask_quantity_mt=sum(
            (Decimal(str(row.remaining_quantity_mt)) for row in asks), Decimal("0")
        ),
        demo_status=demo_status,
        benchmark_usd_per_mt=benchmark_price,
        observed_at=now,
        note=_slice_note(demo_status, has_benchmark=quote is not None),
    )

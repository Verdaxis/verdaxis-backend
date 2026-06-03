"""Generate disclosed demo market activity for the demo phase."""

from __future__ import annotations

import random
from datetime import datetime, timedelta, UTC
from decimal import Decimal
from uuid import UUID

from sqlalchemy import delete, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import Initiator, OrderBookOrder, OrderBookStatus, OrderSide, Trade, TradeStatus
from app.models.user import OrgType, Organization, TierLabel
from app.seeds.market_seed import CI_DATA, PRICING, _slice_certification_scheme
from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window
from app.services.demo_market import (
    DEMO_ACTIVITY_BUYER_ORG_ID,
    DEMO_ACTIVITY_ORG_IDS,
    DEMO_ACTIVITY_SELLER_ORG_ID,
)

ACTIVITY_WINDOWS = (SPOT_WINDOW, "2026-06", "2026-Q3")
MAX_GENERATED_TRADES = 80
MAX_GENERATED_ORDERS = MAX_GENERATED_TRADES * 3
RETENTION_DAYS = 7

_RNG = random.Random()


def _activity_tick(now: datetime) -> datetime:
    reference = now if now.tzinfo else now.replace(tzinfo=UTC)
    return reference.replace(minute=(reference.minute // 5) * 5, second=0, microsecond=0)


async def ensure_demo_activity_organizations(db: AsyncSession) -> None:
    organizations = {
        DEMO_ACTIVITY_BUYER_ORG_ID: Organization(
            id=DEMO_ACTIVITY_BUYER_ORG_ID,
            name="Verdaxis Demo Buyer Activity",
            domain="demo-buyer.verdaxis.local",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        ),
        DEMO_ACTIVITY_SELLER_ORG_ID: Organization(
            id=DEMO_ACTIVITY_SELLER_ORG_ID,
            name="Verdaxis Demo Supplier Activity",
            domain="demo-supplier.verdaxis.local",
            type=OrgType.FUEL_SUPPLIER,
            supplier_tier=TierLabel.REGIONAL_SUPPLIER,
            verification_status="APPROVED",
        ),
    }
    for org in organizations.values():
        await db.merge(org)


def _decimal(value: float | int | str) -> Decimal:
    return Decimal(str(value)).quantize(Decimal("0.01"))


def _quantity() -> Decimal:
    return Decimal(str(_RNG.choice([500, 750, 1000, 1250, 1500, 2000, 2500])))


def _activity_slice(now: datetime) -> tuple[str, str, str]:
    product_name = _RNG.choice(tuple(PRICING.keys()))
    port_name = _RNG.choice(tuple(PRICING[product_name].keys()))
    window = _RNG.choice(ACTIVITY_WINDOWS)
    return product_name, port_name, normalize_availability_window(window)


def _price(product_name: str, port_name: str, side: OrderSide) -> Decimal:
    bid_lo, bid_hi, ask_lo, ask_hi = PRICING[product_name][port_name]
    lo, hi = (bid_lo, bid_hi) if side == OrderSide.BID else (ask_lo, ask_hi)
    return _decimal(round(_RNG.uniform(lo, hi), 2))


def _ask_metadata(product_name: str, port_name: str, window: str) -> dict[str, object]:
    ci_lo, ci_hi, energy_density = CI_DATA[product_name]
    scheme = _slice_certification_scheme(product_name, port_name, window)
    return {
        "certifications": [scheme],
        "certification_declared": True,
        "certification_scheme": scheme,
        "specification_standard": "ISO 8217 / supplier specification",
        "msds_available": True,
        "is_verdaxis_verified": True,
        "carbon_intensity_gco2_mj": _decimal(round(_RNG.uniform(ci_lo, ci_hi), 2)),
        "carbon_intensity_method": "ISCC EU default pathway",
        "energy_density_mj_kg": _decimal(energy_density),
        "feedstock": "Demo feedstock disclosure",
        "origin": f"{port_name} demo terminal",
        "off_spec": False,
    }


async def _load_product_and_port(
    db: AsyncSession, product_name: str, port_name: str
) -> tuple[Product | None, DeliveryPoint | None]:
    product = (
        await db.execute(select(Product).where(Product.name == product_name))
    ).scalar_one_or_none()
    delivery_point = (
        await db.execute(select(DeliveryPoint).where(DeliveryPoint.name == port_name))
    ).scalar_one_or_none()
    return product, delivery_point


async def _delete_trades_and_linked_orders(
    db: AsyncSession, trade_rows: list[tuple[UUID, UUID | None, UUID | None]]
) -> tuple[int, int]:
    if not trade_rows:
        return 0, 0

    trade_ids = [row[0] for row in trade_rows]
    order_ids = {order_id for _, bid_order_id, ask_order_id in trade_rows for order_id in (bid_order_id, ask_order_id) if order_id}

    await db.execute(delete(Trade).where(Trade.id.in_(trade_ids)))
    if order_ids:
        deleted_orders = (
            await db.execute(
                delete(OrderBookOrder)
                .where(OrderBookOrder.id.in_(order_ids))
                .returning(OrderBookOrder.id)
            )
        ).all()
    else:
        deleted_orders = []

    return len(trade_ids), len(deleted_orders)


def _unreferenced_demo_order_filter():
    referenced_bid_orders = select(Trade.bid_order_id).where(Trade.bid_order_id.is_not(None))
    referenced_ask_orders = select(Trade.ask_order_id).where(Trade.ask_order_id.is_not(None))
    return (
        OrderBookOrder.organization_id.in_(DEMO_ACTIVITY_ORG_IDS),
        OrderBookOrder.id.not_in(referenced_bid_orders),
        OrderBookOrder.id.not_in(referenced_ask_orders),
    )


async def prune_demo_activity(db: AsyncSession, *, now: datetime | None = None) -> dict[str, int]:
    reference = now or datetime.now(UTC)
    cutoff = reference - timedelta(days=RETENTION_DAYS)

    old_trade_rows = (
        await db.execute(
            select(Trade.id, Trade.bid_order_id, Trade.ask_order_id)
            .where(
                Trade.buyer_id.in_(DEMO_ACTIVITY_ORG_IDS),
                Trade.seller_id.in_(DEMO_ACTIVITY_ORG_IDS),
                Trade.created_at < cutoff,
            )
        )
    ).all()
    trades_pruned, linked_orders_pruned = await _delete_trades_and_linked_orders(db, old_trade_rows)

    old_orders = (
        await db.execute(
            delete(OrderBookOrder)
            .where(
                *_unreferenced_demo_order_filter(),
                OrderBookOrder.created_at < cutoff,
            )
            .returning(OrderBookOrder.id)
        )
    ).all()

    total_trades = (
        await db.execute(
            select(func.count()).where(
                Trade.buyer_id.in_(DEMO_ACTIVITY_ORG_IDS),
                Trade.seller_id.in_(DEMO_ACTIVITY_ORG_IDS),
            )
        )
    ).scalar() or 0
    capped_trades = 0
    capped_linked_orders = 0
    if total_trades > MAX_GENERATED_TRADES:
        stale_trade_rows = (
            await db.execute(
                select(Trade.id, Trade.bid_order_id, Trade.ask_order_id)
                .where(
                    Trade.buyer_id.in_(DEMO_ACTIVITY_ORG_IDS),
                    Trade.seller_id.in_(DEMO_ACTIVITY_ORG_IDS),
                )
                .order_by(Trade.created_at.asc())
                .limit(total_trades - MAX_GENERATED_TRADES)
            )
        ).all()
        capped_trades, capped_linked_orders = await _delete_trades_and_linked_orders(db, stale_trade_rows)

    total_orders = (
        await db.execute(
            select(func.count()).where(OrderBookOrder.organization_id.in_(DEMO_ACTIVITY_ORG_IDS))
        )
    ).scalar() or 0
    capped_orders = []
    if total_orders > MAX_GENERATED_ORDERS:
        stale_order_ids = (
            await db.execute(
                select(OrderBookOrder.id)
                .where(*_unreferenced_demo_order_filter())
                .order_by(OrderBookOrder.created_at.asc())
                .limit(total_orders - MAX_GENERATED_ORDERS)
            )
        ).scalars().all()
        if stale_order_ids:
            capped_orders = (
                await db.execute(
                    delete(OrderBookOrder)
                    .where(OrderBookOrder.id.in_(stale_order_ids))
                    .returning(OrderBookOrder.id)
                )
            ).all()

    return {
        "trades_pruned": trades_pruned + capped_trades,
        "orders_pruned": linked_orders_pruned + len(old_orders) + capped_linked_orders + len(capped_orders),
    }


async def generate_demo_market_activity(
    db: AsyncSession, *, now: datetime | None = None
) -> dict[str, object]:
    """Create one visible demo order and one confirmed demo trade."""
    reference = _activity_tick(now or datetime.now(UTC))
    tick_key = int(reference.timestamp() // 300)
    _RNG.seed(tick_key)

    await ensure_demo_activity_organizations(db)
    prune_result = await prune_demo_activity(db, now=reference)

    existing_tick_trade = (
        await db.execute(
            select(Trade.id).where(
                Trade.buyer_id == DEMO_ACTIVITY_BUYER_ORG_ID,
                Trade.seller_id == DEMO_ACTIVITY_SELLER_ORG_ID,
                Trade.created_at == reference - timedelta(minutes=3),
            )
        )
    ).scalar_one_or_none()
    if existing_tick_trade:
        await db.commit()
        return {
            "created_orders": 0,
            "created_trades": 0,
            "reason": "already generated for tick",
            **prune_result,
        }

    product_name, port_name, window = _activity_slice(reference)
    product, delivery_point = await _load_product_and_port(db, product_name, port_name)
    if product is None or delivery_point is None:
        return {
            "created_orders": 0,
            "created_trades": 0,
            "reason": "missing product or delivery point",
            **prune_result,
        }

    visible_side = OrderSide.BID if int(reference.timestamp() // 300) % 2 == 0 else OrderSide.ASK
    visible_qty = _quantity()
    visible_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_BUYER_ORG_ID if visible_side == OrderSide.BID else DEMO_ACTIVITY_SELLER_ORG_ID,
        side=visible_side,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=visible_qty,
        remaining_quantity_mt=visible_qty,
        price_per_mt_usd=_price(product_name, port_name, visible_side),
        availability_window=window,
        status=OrderBookStatus.OPEN,
        created_at=reference,
        updated_at=reference,
        **(_ask_metadata(product_name, port_name, window) if visible_side == OrderSide.ASK else {}),
    )
    db.add(visible_order)

    trade_qty = _quantity()
    bid_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_BUYER_ORG_ID,
        side=OrderSide.BID,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=trade_qty,
        remaining_quantity_mt=Decimal("0"),
        price_per_mt_usd=_price(product_name, port_name, OrderSide.BID),
        availability_window=window,
        status=OrderBookStatus.FILLED,
        created_at=reference - timedelta(minutes=8),
        updated_at=reference,
    )
    ask_price = max(_price(product_name, port_name, OrderSide.ASK), bid_order.price_per_mt_usd)
    ask_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=trade_qty,
        remaining_quantity_mt=Decimal("0"),
        price_per_mt_usd=ask_price,
        availability_window=window,
        status=OrderBookStatus.FILLED,
        created_at=reference - timedelta(minutes=7),
        updated_at=reference,
        **_ask_metadata(product_name, port_name, window),
    )
    db.add_all([bid_order, ask_order])
    await db.flush()

    trade = Trade(
        bid_order_id=bid_order.id,
        ask_order_id=ask_order.id,
        buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
        seller_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        initiated_by=Initiator.BUYER,
        is_anonymous=True,
        quantity_mt=trade_qty,
        price_per_mt_usd=ask_price,
        status=TradeStatus.CONFIRMED,
        confirmed_at=reference - timedelta(minutes=2),
        created_at=reference - timedelta(minutes=3),
    )
    db.add(trade)
    await db.commit()

    return {
        "created_orders": 3,
        "created_trades": 1,
        "product": product_name,
        "delivery_point": port_name,
        "availability_window": window,
        **prune_result,
    }

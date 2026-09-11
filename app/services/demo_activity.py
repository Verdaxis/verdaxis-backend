"""Generate disclosed demo market activity for the demo phase."""

from __future__ import annotations

import random
from hashlib import sha256
from datetime import datetime, timedelta, UTC
from decimal import Decimal
from uuid import UUID

from sqlalchemy import delete, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product
from app.demo_identities import DEMO_SEED_BUYERS, DEMO_SEED_SUPPLIERS
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderCreationMethod,
    OrderSide,
    Trade,
)
from app.models.user import OrgType, Organization, OrganizationProvenance, TierLabel
from app.seeds.catalog_seed import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.seeds.market_seed import (
    CI_DATA,
    PRICING,
    _seed_price_for_slice,
    _slice_certification_scheme,
)
from app.services.availability_windows import (
    availability_window_expiry,
    normalize_availability_window,
    tradable_availability_windows,
)
from app.services.demo_market import (
    DEMO_ACTIVITY_BUYER_ORG_ID,
    DEMO_ACTIVITY_ORG_IDS,
    DEMO_ACTIVITY_SELLER_ORG_ID,
)
from app.services.matching_engine import match_order
from app.services.market_admission import lock_and_load_market_organizations
from app.services.market_locks import acquire_market_slice_lock

MAX_GENERATED_TRADES = 80
MAX_GENERATED_ORDERS = MAX_GENERATED_TRADES * 3
RETENTION_DAYS = 7
DEMO_COVERAGE_OPERATION = "DEMO_COVERAGE"
DEMO_COVERAGE_LEVELS_PER_SIDE = 28  # Preserve near-term depth across 20 forward quarters.
DEMO_COVERAGE_REFRESH_FIELDS = (
    "creation_method",
    "quantity_mt",
    "remaining_quantity_mt",
    "price_per_mt_usd",
    "certifications",
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "is_verdaxis_verified",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "energy_density_mj_kg",
    "feedstock",
    "origin",
    "off_spec",
    "status",
    "expires_at",
    "idempotency_request_hash",
)

_RNG = random.Random()


def _activity_tick(now: datetime) -> datetime:
    reference = now if now.tzinfo else now.replace(tzinfo=UTC)
    return reference.replace(minute=(reference.minute // 5) * 5, second=0, microsecond=0)


def activity_windows(now: datetime) -> tuple[str, ...]:
    """Current canonical windows, derived from the injected activity clock."""
    reference = now if now.tzinfo else now.replace(tzinfo=UTC)
    return tuple(tradable_availability_windows(today=reference.date()))


def build_demo_market_coverage(now: datetime) -> list[OrderBookOrder]:
    """Build the rolling disclosed baseline without touching historical rows."""
    reference = now if now.tzinfo else now.replace(tzinfo=UTC)
    reference = reference.astimezone(UTC)
    buyer_ids = tuple(organization_id for organization_id, _name in DEMO_SEED_BUYERS)
    supplier_ids = tuple(
        organization_id for organization_id, _name, _tier in DEMO_SEED_SUPPLIERS
    )
    orders: list[OrderBookOrder] = []
    windows = activity_windows(reference)
    second_level_windows = set(
        windows[: max(DEMO_COVERAGE_LEVELS_PER_SIDE - len(windows), 0)]
    )

    for product_name, ports in PRICING.items():
        ci_lo, ci_hi, energy_density = CI_DATA[product_name]
        for port_name, (bid_lo, bid_hi, ask_lo, ask_hi) in ports.items():
            for window in windows:
                depth = 2 if window in second_level_windows else 1
                scheme = _slice_certification_scheme(product_name, port_name, window)
                for side in (OrderSide.BID, OrderSide.ASK):
                    for depth_index in range(depth):
                        ordinal = len(orders)
                        quantity = Decimal("1500") if depth_index == 0 else Decimal("1000")
                        key = (
                            f"{product_name}|{port_name}|{window}|"
                            f"{side.value}|{depth_index}"
                        )
                        values: dict[str, object] = {
                            "organization_id": (
                                buyer_ids[ordinal % len(buyer_ids)]
                                if side == OrderSide.BID
                                else supplier_ids[ordinal % len(supplier_ids)]
                            ),
                            "creation_method": OrderCreationMethod.SYSTEM,
                            "provenance": OrganizationProvenance.DEMO,
                            "side": side,
                            "product_id": PRODUCT_IDS[product_name],
                            "delivery_point_id": DELIVERY_POINT_IDS[port_name],
                            "quantity_mt": quantity,
                            "remaining_quantity_mt": quantity,
                            "price_per_mt_usd": _seed_price_for_slice(
                                side,
                                bid_lo=bid_lo,
                                bid_hi=bid_hi,
                                ask_lo=ask_lo,
                                ask_hi=ask_hi,
                                window=window,
                                depth_index=depth_index,
                            ),
                            "availability_window": window,
                            "certifications": [],
                            "certification_declared": False,
                            "certification_scheme": scheme,
                            "specification_standard": None,
                            "msds_available": False,
                            "is_verdaxis_verified": False,
                            "carbon_intensity_gco2_mj": None,
                            "carbon_intensity_method": None,
                            "energy_density_mj_kg": None,
                            "feedstock": None,
                            "origin": None,
                            "off_spec": False,
                            "status": OrderBookStatus.OPEN,
                            "expires_at": availability_window_expiry(
                                window, observed_at=reference
                            ),
                            "idempotency_operation": DEMO_COVERAGE_OPERATION,
                            "idempotency_key": key,
                            "idempotency_request_hash": sha256(
                                key.encode("utf-8")
                            ).hexdigest(),
                            "created_at": reference
                            - timedelta(minutes=ordinal % 360),
                            "updated_at": reference,
                        }
                        if side == OrderSide.ASK:
                            values.update(
                                {
                                    "certifications": [scheme],
                                    "certification_declared": True,
                                    "specification_standard": "Supplier specification",
                                    "msds_available": True,
                                    "is_verdaxis_verified": True,
                                    "carbon_intensity_gco2_mj": _decimal(
                                        (ci_lo + ci_hi) / 2
                                    ),
                                    "carbon_intensity_method": "Indicative demo pathway",
                                    "energy_density_mj_kg": _decimal(energy_density),
                                    "feedstock": "Indicative demo feedstock",
                                    "origin": f"{port_name} demo terminal",
                                    "off_spec": False,
                                }
                            )
                        orders.append(OrderBookOrder(**values))

    return orders


async def ensure_demo_market_coverage(
    db: AsyncSession, *, now: datetime | None = None
) -> dict[str, int]:
    """Create or refresh the current canonical demo book idempotently."""
    reference = now or datetime.now(UTC)
    desired = build_demo_market_coverage(reference)
    existing = (
        await db.execute(
            select(OrderBookOrder).where(
                OrderBookOrder.provenance == OrganizationProvenance.DEMO,
                OrderBookOrder.idempotency_operation == DEMO_COVERAGE_OPERATION,
            )
        )
    ).scalars().all()
    existing_by_key = {order.idempotency_key: order for order in existing}
    desired_keys = {order.idempotency_key for order in desired}
    created = 0
    refreshed = 0

    for target in desired:
        current = existing_by_key.get(target.idempotency_key)
        if current is None:
            db.add(target)
            created += 1
            continue

        changed = False
        for field in DEMO_COVERAGE_REFRESH_FIELDS:
            value = getattr(target, field)
            if getattr(current, field) != value:
                setattr(current, field, value)
                changed = True
        if changed:
            current.updated_at = reference
            refreshed += 1

    expired = 0
    for order in existing:
        if (
            order.idempotency_key not in desired_keys
            and order.status in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
        ):
            order.status = OrderBookStatus.EXPIRED
            order.updated_at = reference
            expired += 1

    return {
        "coverage_orders": len(desired),
        "coverage_created": created,
        "coverage_refreshed": refreshed,
        "coverage_expired": expired,
    }


async def _tick_trade_exists(db: AsyncSession, reference: datetime) -> bool:
    trade_id = (
        await db.execute(
            select(Trade.id).where(
                Trade.buyer_id == DEMO_ACTIVITY_BUYER_ORG_ID,
                Trade.seller_id == DEMO_ACTIVITY_SELLER_ORG_ID,
                Trade.created_at == reference - timedelta(minutes=3),
            )
        )
    ).scalar_one_or_none()
    return trade_id is not None


async def ensure_demo_activity_organizations(db: AsyncSession) -> None:
    organizations = {
        DEMO_ACTIVITY_BUYER_ORG_ID: Organization(
            id=DEMO_ACTIVITY_BUYER_ORG_ID,
            name="Verdaxis Demo Buyer Activity",
            domain="demo-buyer.verdaxis.local",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
            provenance=OrganizationProvenance.DEMO,
        ),
        DEMO_ACTIVITY_SELLER_ORG_ID: Organization(
            id=DEMO_ACTIVITY_SELLER_ORG_ID,
            name="Verdaxis Demo Supplier Activity",
            domain="demo-supplier.verdaxis.local",
            type=OrgType.FUEL_SUPPLIER,
            supplier_tier=TierLabel.REGIONAL_SUPPLIER,
            verification_status="APPROVED",
            provenance=OrganizationProvenance.DEMO,
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
    reference = now if now.tzinfo else now.replace(tzinfo=UTC)
    window = _RNG.choice(activity_windows(reference))
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
        await db.execute(
            select(Product).where(
                Product.name == product_name,
                Product.is_active.is_(True),
            )
        )
    ).scalar_one_or_none()
    delivery_point = (
        await db.execute(
            select(DeliveryPoint).where(
                DeliveryPoint.name == port_name,
                DeliveryPoint.is_active.is_(True),
            )
        )
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

    # Organization provisioning and retention pruning are separate maintenance
    # transactions. This transaction owns only one canonical market slice.
    prune_result = {"trades_pruned": 0, "orders_pruned": 0}

    if await _tick_trade_exists(db, reference):
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
    await acquire_market_slice_lock(
        db,
        side=OrderSide.BID,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        availability_window=window,
    )
    # A concurrent tick may have passed the fast pre-check before waiting on
    # this slice. Recheck under the transaction lock before inserting.
    if await _tick_trade_exists(db, reference):
        return {
            "created_orders": 0,
            "created_trades": 0,
            "reason": "already generated for tick",
            **prune_result,
        }
    await lock_and_load_market_organizations(db, DEMO_ACTIVITY_ORG_IDS)

    trade_qty = _quantity()
    ask_price = _price(product_name, port_name, OrderSide.ASK)
    ask_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        provenance=OrganizationProvenance.DEMO,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=trade_qty,
        remaining_quantity_mt=trade_qty,
        price_per_mt_usd=ask_price,
        availability_window=window,
        status=OrderBookStatus.OPEN,
        expires_at=availability_window_expiry(window, observed_at=reference),
        created_at=reference - timedelta(minutes=8),
        updated_at=reference - timedelta(minutes=8),
        **_ask_metadata(product_name, port_name, window),
    )
    db.add(ask_order)
    await db.flush()

    bid_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_BUYER_ORG_ID,
        provenance=OrganizationProvenance.DEMO,
        side=OrderSide.BID,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=trade_qty,
        remaining_quantity_mt=trade_qty,
        price_per_mt_usd=max(_price(product_name, port_name, OrderSide.BID), ask_price),
        availability_window=window,
        status=OrderBookStatus.OPEN,
        expires_at=availability_window_expiry(window, observed_at=reference),
        created_at=reference - timedelta(minutes=7),
        updated_at=reference - timedelta(minutes=7),
    )
    db.add(bid_order)
    await db.flush()

    trades = await match_order(
        db,
        bid_order,
        is_anonymous=True,
        allowed_demo_order_pair=frozenset((bid_order.id, ask_order.id)),
    )
    if len(trades) != 1 or trades[0].bid_order_id != bid_order.id or trades[0].ask_order_id != ask_order.id:
        raise RuntimeError(
            "demo pair did not produce exactly one canonical match; caller must roll back"
        )
    trade = trades[0]
    trade.created_at = reference - timedelta(minutes=3)
    trade.confirmed_at = reference - timedelta(minutes=2)
    bid_order.updated_at = reference
    ask_order.updated_at = reference

    visible_side = OrderSide.BID if int(reference.timestamp() // 300) % 2 == 0 else OrderSide.ASK
    visible_qty = _quantity()
    visible_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_BUYER_ORG_ID if visible_side == OrderSide.BID else DEMO_ACTIVITY_SELLER_ORG_ID,
        provenance=OrganizationProvenance.DEMO,
        side=visible_side,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=visible_qty,
        remaining_quantity_mt=visible_qty,
        price_per_mt_usd=_price(product_name, port_name, visible_side),
        availability_window=window,
        status=OrderBookStatus.OPEN,
        expires_at=availability_window_expiry(window, observed_at=reference),
        created_at=reference,
        updated_at=reference,
        **(_ask_metadata(product_name, port_name, window) if visible_side == OrderSide.ASK else {}),
    )
    db.add(visible_order)

    return {
        "created_orders": 3,
        "created_trades": 1,
        "product": product_name,
        "delivery_point": port_name,
        "availability_window": window,
        **prune_result,
    }

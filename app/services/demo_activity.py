"""Generate disclosed demo market activity for the demo phase."""

from __future__ import annotations

import random
from hashlib import sha256
from datetime import datetime, timedelta, UTC
from decimal import Decimal
from uuid import UUID

from sqlalchemy import delete, func, or_, select, update
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product
from app.models.matchmaking import MatchSuggestion
from app.models.negotiation import Negotiation
from app.models.orders import Commission
from app.models.rfq import RFQ
from app.models.watchlist import WatchlistTarget
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
    ask_seed_metadata,
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
DEMO_COVERAGE_LEVELS_PER_SIDE = 10
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
    return tuple(tradable_availability_windows(today=reference.astimezone(UTC).date()))


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

    for product_name, ports in PRICING.items():
        ci_lo, ci_hi, energy_density = CI_DATA[product_name]
        for port_name, (bid_lo, bid_hi, ask_lo, ask_hi) in ports.items():
            for window in windows:
                scheme = _slice_certification_scheme(product_name, port_name, window)
                for side in (OrderSide.BID, OrderSide.ASK):
                    for depth_index in range(DEMO_COVERAGE_LEVELS_PER_SIDE):
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
                                reference_date=reference.date(),
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
                            if product_name in {"B30", "B100"}:
                                values.update(ask_seed_metadata(scheme, product_name=product_name))
                                values["is_verdaxis_verified"] = False
                        orders.append(OrderBookOrder(**values))

    return orders


async def _expire_legacy_activity_quotes(db: AsyncSession, reference: datetime) -> int:
    """Retire old flat-price system quotes while preserving records and pins."""
    referenced_columns = (
        Trade.bid_order_id,
        Trade.ask_order_id,
        Negotiation.bid_order_id,
        Negotiation.ask_order_id,
        MatchSuggestion.bid_order_id,
        MatchSuggestion.ask_order_id,
    )
    expired_ids = (
        await db.execute(
            update(OrderBookOrder)
            .where(
                OrderBookOrder.organization_id.in_(DEMO_ACTIVITY_ORG_IDS),
                OrderBookOrder.provenance == OrganizationProvenance.DEMO,
                OrderBookOrder.creation_method == OrderCreationMethod.LEGACY_UNKNOWN,
                OrderBookOrder.owner_user_id.is_(None),
                OrderBookOrder.created_by_actor_user_id.is_(None),
                OrderBookOrder.support_authorization_id.is_(None),
                OrderBookOrder.idempotency_operation.is_(None),
                OrderBookOrder.idempotency_key.is_(None),
                OrderBookOrder.idempotency_request_hash.is_(None),
                OrderBookOrder.status == OrderBookStatus.OPEN,
                OrderBookOrder.remaining_quantity_mt == OrderBookOrder.quantity_mt,
                *(
                    OrderBookOrder.id.not_in(select(column).where(column.is_not(None)))
                    for column in referenced_columns
                ),
            )
            .values(status=OrderBookStatus.EXPIRED, updated_at=reference)
            .returning(OrderBookOrder.id)
        )
    ).scalars().all()
    return len(expired_ids)


async def ensure_demo_market_coverage(
    db: AsyncSession, *, now: datetime | None = None
) -> dict[str, int]:
    """Create or refresh the current canonical demo book idempotently."""
    reference = now or datetime.now(UTC)
    desired = build_demo_market_coverage(reference)
    desired_keys = {order.idempotency_key for order in desired}
    existing = (
        await db.execute(
            select(OrderBookOrder).where(
                OrderBookOrder.provenance == OrganizationProvenance.DEMO,
                OrderBookOrder.idempotency_operation == DEMO_COVERAGE_OPERATION,
                or_(
                    OrderBookOrder.idempotency_key.in_(desired_keys),
                    OrderBookOrder.status.in_((OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)),
                ),
            )
        )
    ).scalars().all()
    existing_by_key = {order.idempotency_key: order for order in existing}
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

    legacy_activity_expired = await _expire_legacy_activity_quotes(db, reference)
    return {
        "coverage_orders": len(desired),
        "coverage_created": created,
        "coverage_refreshed": refreshed,
        "coverage_expired": expired,
        "legacy_activity_expired": legacy_activity_expired,
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


def _price(product_name: str, port_name: str, window: str, now: datetime) -> Decimal:
    """Price the synthetic matched pair at the current demo book midpoint."""
    bid_lo, bid_hi, ask_lo, ask_hi = PRICING[product_name][port_name]
    prices = [
        _seed_price_for_slice(
            side,
            bid_lo=bid_lo,
            bid_hi=bid_hi,
            ask_lo=ask_lo,
            ask_hi=ask_hi,
            window=window,
            reference_date=now.astimezone(UTC).date(),
        )
        for side in (OrderSide.BID, OrderSide.ASK)
    ]
    return ((prices[0] + prices[1]) / Decimal("2")).quantize(Decimal("0.01"))


def _ask_metadata(product_name: str, port_name: str, window: str) -> dict[str, object]:
    ci_lo, ci_hi, energy_density = CI_DATA[product_name]
    scheme = _slice_certification_scheme(product_name, port_name, window)
    metadata = {
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
    if product_name in {"B30", "B100"}:
        metadata.update(ask_seed_metadata(scheme, product_name=product_name))
        metadata["is_verdaxis_verified"] = False
    return metadata


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

    deleted_trade_ids = set((
        await db.execute(
            delete(Trade)
            .where(Trade.id.in_([row[0] for row in trade_rows]), *_unreferenced_demo_trade_filter())
            .returning(Trade.id)
        )
    ).scalars().all())
    order_ids = {
        order_id
        for trade_id, bid_order_id, ask_order_id in trade_rows
        if trade_id in deleted_trade_ids
        for order_id in (bid_order_id, ask_order_id)
        if order_id is not None
    }
    if order_ids:
        deleted_orders = (
            await db.execute(
                delete(OrderBookOrder)
                .where(OrderBookOrder.id.in_(order_ids), *_unreferenced_demo_order_filter())
                .returning(OrderBookOrder.id)
            )
        ).all()
    else:
        deleted_orders = []

    return len(deleted_trade_ids), len(deleted_orders)


def _unreferenced_demo_order_filter():
    referenced_order_columns = (
        Trade.bid_order_id,
        Trade.ask_order_id,
        WatchlistTarget.order_id,
        MatchSuggestion.bid_order_id,
        MatchSuggestion.ask_order_id,
        Negotiation.bid_order_id,
        Negotiation.ask_order_id,
    )
    return (
        OrderBookOrder.organization_id.in_(DEMO_ACTIVITY_ORG_IDS),
        OrderBookOrder.provenance == OrganizationProvenance.DEMO,
        *(
            OrderBookOrder.id.not_in(select(column).where(column.is_not(None)))
            for column in referenced_order_columns
        ),
    )


def _unreferenced_demo_trade_filter():
    return (
        Trade.buyer_id.in_(DEMO_ACTIVITY_ORG_IDS),
        Trade.seller_id.in_(DEMO_ACTIVITY_ORG_IDS),
        Trade.buyer_provenance == OrganizationProvenance.DEMO,
        Trade.seller_provenance == OrganizationProvenance.DEMO,
        *(
            Trade.id.not_in(select(column).where(column.is_not(None)))
            for column in (Commission.trade_id, RFQ.trade_id, Negotiation.trade_id)
        ),
    )


async def prune_demo_activity(db: AsyncSession, *, now: datetime | None = None) -> dict[str, int]:
    reference = now or datetime.now(UTC)
    cutoff = reference - timedelta(days=RETENTION_DAYS)

    old_trade_rows = (
        await db.execute(
            select(Trade.id, Trade.bid_order_id, Trade.ask_order_id)
            .where(
                *_unreferenced_demo_trade_filter(),
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

    # ponytail: retention caps are soft while customer/history references remain.
    # Referenced records become eligible after those references are removed.
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
                    *_unreferenced_demo_trade_filter(),
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
                    .where(OrderBookOrder.id.in_(stale_order_ids), *_unreferenced_demo_order_filter())
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
    """Create one disclosed matched pair; coverage owns the resting demo book."""
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
    trade_price = _price(product_name, port_name, window, reference)
    ask_order = OrderBookOrder(
        organization_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        provenance=OrganizationProvenance.DEMO,
        creation_method=OrderCreationMethod.SYSTEM,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=trade_qty,
        remaining_quantity_mt=trade_qty,
        price_per_mt_usd=trade_price,
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
        creation_method=OrderCreationMethod.SYSTEM,
        side=OrderSide.BID,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=trade_qty,
        remaining_quantity_mt=trade_qty,
        price_per_mt_usd=trade_price,
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

    return {
        "created_orders": 2,
        "created_trades": 1,
        "product": product_name,
        "delivery_point": port_name,
        "availability_window": window,
        **prune_result,
    }

"""Realistic market seed data — orders, trades, and RFQs.

Populates the orderbook with ~105 orders across all fuel-type/port combos,
~40 matched trades, and ~10 RFQs with quotes.  Prices reflect 2025-2026
marine fuel markets.

Idempotent: checks for a sentinel order before inserting.  Clears old test
data on first run.
"""
import random
import uuid
from datetime import datetime, timedelta, timezone
from decimal import Decimal

from sqlalchemy import text, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import (
    OrderBookOrder,
    Trade,
    OrderSide,
    OrderBookStatus,
    TradeStatus,
    AvailabilityWindow,
    Initiator,
)
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus
from app.seeds.catalog_seed import PRODUCT_IDS, DELIVERY_POINT_IDS

# ---------------------------------------------------------------------------
# Deterministic seed for reproducibility
# ---------------------------------------------------------------------------
_RNG = random.Random(42)

# Sentinel: if an order with this ID exists, seeding already happened.
_SENTINEL_ID = uuid.UUID("00000000-dead-beef-0000-aaa0e15eed01")

# ---------------------------------------------------------------------------
# Fake organization IDs (buyers and suppliers)
# ---------------------------------------------------------------------------
_NS = uuid.UUID("b2c3d4e5-f6a7-8901-bcde-f12345678901")

BUYER_ORGS = [
    {"id": uuid.uuid5(_NS, "buyer:maersk_fuel_procurement"), "name": "Maersk Fuel Procurement"},
    {"id": uuid.uuid5(_NS, "buyer:evergreen_marine_bunkers"), "name": "Evergreen Marine Bunkers"},
    {"id": uuid.uuid5(_NS, "buyer:cosco_energy_trading"), "name": "COSCO Energy Trading"},
    {"id": uuid.uuid5(_NS, "buyer:msc_fuel_desk"), "name": "MSC Fuel Desk"},
    {"id": uuid.uuid5(_NS, "buyer:cma_cgm_green_fuel"), "name": "CMA CGM Green Fuel"},
]

SUPPLIER_ORGS = [
    {"id": uuid.uuid5(_NS, "seller:vitol_bunkers"), "name": "Vitol Bunkers", "tier": "MAJOR_TRADER"},
    {"id": uuid.uuid5(_NS, "seller:trafigura_marine"), "name": "Trafigura Marine", "tier": "MAJOR_TRADER"},
    {"id": uuid.uuid5(_NS, "seller:oci_green_fuels"), "name": "OCI Green Fuels", "tier": "TIER_1_PRODUCER"},
    {"id": uuid.uuid5(_NS, "seller:peninsula_petroleum"), "name": "Peninsula Petroleum", "tier": "REGIONAL_SUPPLIER"},
    {"id": uuid.uuid5(_NS, "seller:bunker_holding_group"), "name": "Bunker Holding Group", "tier": "MAJOR_TRADER"},
]

# ---------------------------------------------------------------------------
# Pricing matrix — product name -> delivery point -> (bid_lo, bid_hi, ask_lo, ask_hi)
# ---------------------------------------------------------------------------
PRICING: dict[str, dict[str, tuple[float, float, float, float]]] = {
    # Based on Ship & Bunker real market data (March 2026)
    # Green methanol = ~2x gray methanol premium
    "Methanol Green": {
        "ARA":       (540, 580, 595, 640),      # Gray ~$292, green premium ~2x
        "Singapore": (1020, 1070, 1090, 1140),   # Gray ~$545, green premium ~2x
        "Fujairah":  (640, 680, 700, 745),        # Gray ~$339, green premium ~2x
        "Rotterdam": (545, 585, 600, 645),
    },
    # VLSFO — real market: ROT $757, SG $894, FUJ $941
    "VLSFO Conventional": {
        "ARA":       (730, 755, 765, 795),
        "Singapore": (870, 895, 905, 935),
        "Fujairah":  (915, 940, 950, 980),
    },
    # HVO biofuel — typically $100-200 premium over VLSFO
    "Biofuel Bio": {
        "ARA":       (870, 910, 925, 965),
        "Singapore": (990, 1040, 1055, 1100),
        "Fujairah":  (1020, 1065, 1080, 1125),
    },
    # Green ammonia — nascent market, projected range
    "Ammonia Green": {
        "ARA":       (620, 670, 690, 740),
        "Singapore": (680, 730, 750, 800),
    },
    # LNG bunker — real market equivalent
    "LNG Conventional": {
        "ARA":       (700, 740, 755, 800),
        "Singapore": (750, 795, 810, 855),
    },
    # MGO — real market: ROT $1334, SG $1784, FUJ $1640
    "MGO Conventional": {
        "ARA":       (1300, 1335, 1350, 1385),
        "Singapore": (1745, 1785, 1800, 1840),
        "Fujairah":  (1600, 1640, 1655, 1695),
    },
}

# CI data ranges per product: (ci_lo, ci_hi, energy_density)
CI_DATA: dict[str, tuple[float, float, float]] = {
    "Methanol Green":     (70, 98, 19.9),
    "VLSFO Conventional": (40, 60, 40.2),
    "Biofuel Bio":        (55, 85, 44.0),
    "Ammonia Green":      (75, 98, 18.6),
    "LNG Conventional":   (42, 58, 48.6),
    "MGO Conventional":   (38, 55, 42.7),
}

WINDOWS = [
    AvailabilityWindow.SPOT,
    AvailabilityWindow.Q1_2026,
    AvailabilityWindow.Q2_2026,
    AvailabilityWindow.Q3_2026,
    AvailabilityWindow.Q4_2026,
]

QUANTITIES = [500, 1000, 1500, 2000, 2500, 3000, 3500, 4000, 4500, 5000]

# RFQ notes templates
RFQ_NOTES = [
    "Seeking competitive quotes for upcoming voyage refueling",
    "Annual contract renewal — need firm offers for next quarter",
    "Spot requirement, flexible on delivery window +/- 5 days",
    "Looking for ISCC-certified supply only",
    "Fleet-wide procurement, multiple deliveries expected",
    "Urgent requirement — vessel arriving next week",
    "Testing new fuel pathway for FuelEU compliance",
    "Require full chain-of-custody documentation",
    "Prefer suppliers with ARA barge delivery capability",
    "Need blending options — open to partial bio blends",
]

QUOTE_NOTES = [
    "Ex-tank ARA, loading within 3 days of confirmation",
    "Delivered by barge, ISCC certified",
    "Price valid for 48 hours",
    "Can offer in 500MT increments",
    "Full documentation package included",
    "Subject to final credit check",
]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _price(lo: float, hi: float) -> Decimal:
    return Decimal(str(round(_RNG.uniform(lo, hi), 2)))


def _qty() -> Decimal:
    return Decimal(str(_RNG.choice(QUANTITIES)))


def _window() -> AvailabilityWindow:
    return _RNG.choice(WINDOWS)


def _rand_date(start: datetime, end: datetime) -> datetime:
    delta = end - start
    offset = _RNG.randint(0, int(delta.total_seconds()))
    return start + timedelta(seconds=offset)


# ---------------------------------------------------------------------------
# Core seed function
# ---------------------------------------------------------------------------

async def seed_market_data(db: AsyncSession) -> None:
    """Seed realistic market data. Idempotent — skips if sentinel exists."""

    # Check sentinel
    existing = (await db.execute(
        select(OrderBookOrder.id).where(OrderBookOrder.id == _SENTINEL_ID)
    )).scalar_one_or_none()

    if existing is not None:
        print("[market_seed] Sentinel found — already seeded, skipping.")
        return

    # ------------------------------------------------------------------
    # Step 0: Clear old test data (reverse FK order)
    # ------------------------------------------------------------------
    print("[market_seed] Clearing old test data...")
    await db.execute(text("DELETE FROM commissions"))
    await db.execute(text("DELETE FROM match_suggestions"))
    await db.execute(text("DELETE FROM rfq_quotes"))
    await db.execute(text("DELETE FROM rfqs"))
    await db.execute(text("DELETE FROM trades"))
    await db.execute(text("DELETE FROM orderbook_orders"))
    await db.flush()

    # ------------------------------------------------------------------
    # Step 1: Ensure seed organizations exist
    # ------------------------------------------------------------------
    print("[market_seed] Ensuring seed organizations...")
    for org in BUYER_ORGS:
        await db.execute(text(
            "INSERT INTO organizations (id, name, type, verification_status) "
            "VALUES (:id, :name, 'SHIPPING_LINE', 'APPROVED') "
            "ON CONFLICT (id) DO NOTHING"
        ), {"id": org["id"], "name": org["name"]})

    for org in SUPPLIER_ORGS:
        await db.execute(text(
            "INSERT INTO organizations (id, name, type, supplier_tier, verification_status) "
            "VALUES (:id, :name, 'FUEL_SUPPLIER', :tier, 'APPROVED') "
            "ON CONFLICT (id) DO NOTHING"
        ), {"id": org["id"], "name": org["name"], "tier": org["tier"]})

    await db.flush()

    # ------------------------------------------------------------------
    # Step 2: Create orders
    # ------------------------------------------------------------------
    print("[market_seed] Creating orders...")
    all_orders: list[OrderBookOrder] = []
    orders_by_product_port: dict[str, list[OrderBookOrder]] = {}

    for product_name, ports in PRICING.items():
        product_id = PRODUCT_IDS[product_name]
        ci_lo, ci_hi, energy_density = CI_DATA[product_name]

        for port_name, (bid_lo, bid_hi, ask_lo, ask_hi) in ports.items():
            dp_id = DELIVERY_POINT_IDS[port_name]
            key = f"{product_name}|{port_name}"
            orders_by_product_port[key] = []

            # 3-5 BID orders
            n_bids = _RNG.randint(3, 5)
            for _ in range(n_bids):
                buyer = _RNG.choice(BUYER_ORGS)
                qty = _qty()
                status = _RNG.choices(
                    [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED],
                    weights=[85, 15],
                )[0]
                remaining = qty if status == OrderBookStatus.OPEN else Decimal(str(
                    round(float(qty) * _RNG.uniform(0.2, 0.8) / 500) * 500
                ))
                if remaining <= 0:
                    remaining = Decimal("500")

                created = _rand_date(
                    datetime(2025, 1, 1, tzinfo=timezone.utc),
                    datetime(2025, 3, 20, tzinfo=timezone.utc),
                )

                order = OrderBookOrder(
                    id=uuid.uuid4(),
                    organization_id=buyer["id"],
                    side=OrderSide.BID,
                    product_id=product_id,
                    delivery_point_id=dp_id,
                    quantity_mt=qty,
                    remaining_quantity_mt=remaining,
                    price_per_mt_usd=_price(bid_lo, bid_hi),
                    availability_window=_window(),
                    status=status,
                    created_at=created,
                    updated_at=created,
                )
                db.add(order)
                all_orders.append(order)
                orders_by_product_port[key].append(order)

            # 3-5 ASK orders
            n_asks = _RNG.randint(3, 5)
            for _ in range(n_asks):
                supplier = _RNG.choice(SUPPLIER_ORGS)
                qty = _qty()
                status = _RNG.choices(
                    [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED],
                    weights=[85, 15],
                )[0]
                remaining = qty if status == OrderBookStatus.OPEN else Decimal(str(
                    round(float(qty) * _RNG.uniform(0.2, 0.8) / 500) * 500
                ))
                if remaining <= 0:
                    remaining = Decimal("500")

                ci_value = Decimal(str(round(_RNG.uniform(ci_lo, ci_hi), 2)))
                certs = _RNG.choices(
                    [["ISCC"], ["RSB"], ["ISCC", "RSB"], []],
                    weights=[30, 20, 15, 35],
                )[0]

                created = _rand_date(
                    datetime(2025, 1, 1, tzinfo=timezone.utc),
                    datetime(2025, 3, 20, tzinfo=timezone.utc),
                )

                order = OrderBookOrder(
                    id=uuid.uuid4(),
                    organization_id=supplier["id"],
                    side=OrderSide.ASK,
                    product_id=product_id,
                    delivery_point_id=dp_id,
                    quantity_mt=qty,
                    remaining_quantity_mt=remaining,
                    price_per_mt_usd=_price(ask_lo, ask_hi),
                    availability_window=_window(),
                    status=status,
                    certifications=certs,
                    is_verdaxis_verified=_RNG.random() < 0.3,
                    carbon_intensity_gco2_mj=ci_value,
                    energy_density_mj_kg=Decimal(str(energy_density)),
                    created_at=created,
                    updated_at=created,
                )
                db.add(order)
                all_orders.append(order)
                orders_by_product_port[key].append(order)

    # Insert sentinel order (hidden — CANCELLED, qty 0)
    sentinel = OrderBookOrder(
        id=_SENTINEL_ID,
        organization_id=BUYER_ORGS[0]["id"],
        side=OrderSide.BID,
        product_id=PRODUCT_IDS["VLSFO Conventional"],
        delivery_point_id=DELIVERY_POINT_IDS["ARA"],
        quantity_mt=Decimal("0"),
        remaining_quantity_mt=Decimal("0"),
        price_per_mt_usd=Decimal("0"),
        availability_window=AvailabilityWindow.SPOT,
        status=OrderBookStatus.CANCELLED,
        created_at=datetime(2020, 1, 1, tzinfo=timezone.utc),
        updated_at=datetime(2020, 1, 1, tzinfo=timezone.utc),
    )
    db.add(sentinel)
    await db.flush()

    total_orders = len(all_orders)
    print(f"[market_seed] Created {total_orders} orders + sentinel.")

    # ------------------------------------------------------------------
    # Step 3: Create trades from crossed orders
    # ------------------------------------------------------------------
    print("[market_seed] Creating trades...")
    trades_created = 0
    target_trades = 40

    trade_statuses = [
        TradeStatus.CONFIRMED,
        TradeStatus.DELIVERED,
        TradeStatus.PAID,
    ]
    trade_status_weights = [30, 35, 35]

    # Collect matchable pairs: crossed first, then negotiable (spread < 8%)
    crossed_pairs: list[tuple[OrderBookOrder, OrderBookOrder]] = []
    negotiated_pairs: list[tuple[OrderBookOrder, OrderBookOrder]] = []
    for key, orders in orders_by_product_port.items():
        bids = [o for o in orders if o.side == OrderSide.BID]
        asks = [o for o in orders if o.side == OrderSide.ASK]
        for bid in bids:
            for ask in asks:
                if bid.price_per_mt_usd >= ask.price_per_mt_usd:
                    crossed_pairs.append((bid, ask))
                else:
                    # Allow "negotiated" trades where spread is small
                    mid = (float(bid.price_per_mt_usd) + float(ask.price_per_mt_usd)) / 2
                    spread_pct = (float(ask.price_per_mt_usd) - float(bid.price_per_mt_usd)) / mid
                    if spread_pct < 0.08:
                        negotiated_pairs.append((bid, ask))

    _RNG.shuffle(crossed_pairs)
    _RNG.shuffle(negotiated_pairs)
    # Prioritize crossed (aggressive fills), then negotiated
    all_pairs = crossed_pairs + negotiated_pairs

    used_pairs: set[tuple[uuid.UUID, uuid.UUID]] = set()
    for bid, ask in all_pairs:
        if trades_created >= target_trades:
            break
        pair_key = (bid.id, ask.id)
        if pair_key in used_pairs:
            continue
        # Ensure orgs are different
        if bid.organization_id == ask.organization_id:
            continue
        used_pairs.add(pair_key)

        trade_qty = Decimal(str(_RNG.choice([500, 1000, 1500, 2000])))
        trade_qty = min(trade_qty, bid.remaining_quantity_mt, ask.remaining_quantity_mt)
        if trade_qty <= 0:
            continue

        # Crossed: trade at ask price; Negotiated: trade at midpoint
        if bid.price_per_mt_usd >= ask.price_per_mt_usd:
            trade_price = ask.price_per_mt_usd
        else:
            midpoint = (bid.price_per_mt_usd + ask.price_per_mt_usd) / 2
            trade_price = midpoint.quantize(Decimal("0.01"))
        status = _RNG.choices(trade_statuses, weights=trade_status_weights)[0]

        created = _rand_date(
            datetime(2025, 1, 5, tzinfo=timezone.utc),
            datetime(2025, 3, 15, tzinfo=timezone.utc),
        )
        confirmed = created + timedelta(hours=_RNG.randint(1, 48))
        delivered = confirmed + timedelta(days=_RNG.randint(3, 21)) if status in (
            TradeStatus.DELIVERED, TradeStatus.PAID
        ) else None
        paid = delivered + timedelta(days=_RNG.randint(7, 30)) if status == TradeStatus.PAID and delivered else None

        total_usd = trade_qty * trade_price
        commission_rate = Decimal("0.500")
        commission_amt = (total_usd * commission_rate / Decimal("100")).quantize(Decimal("0.01"))

        trade = Trade(
            id=uuid.uuid4(),
            bid_order_id=bid.id,
            ask_order_id=ask.id,
            buyer_id=bid.organization_id,
            seller_id=ask.organization_id,
            initiated_by=_RNG.choice([Initiator.BUYER, Initiator.SELLER]),
            quantity_mt=trade_qty,
            price_per_mt_usd=trade_price,
            status=status,
            final_quantity_mt=trade_qty if status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None,
            final_price_per_mt=trade_price if status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None,
            final_total_usd=total_usd if status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None,
            commission_rate_pct=commission_rate,
            commission_amount_usd=commission_amt if status == TradeStatus.PAID else None,
            confirmed_at=confirmed,
            delivered_at=delivered,
            paid_at=paid,
            created_at=created,
        )
        db.add(trade)
        trades_created += 1

        # Debit remaining quantities
        bid.remaining_quantity_mt -= trade_qty
        ask.remaining_quantity_mt -= trade_qty
        if bid.remaining_quantity_mt <= 0:
            bid.status = OrderBookStatus.FILLED
            bid.remaining_quantity_mt = Decimal("0")
        elif bid.remaining_quantity_mt < bid.quantity_mt:
            bid.status = OrderBookStatus.PARTIALLY_FILLED
        if ask.remaining_quantity_mt <= 0:
            ask.status = OrderBookStatus.FILLED
            ask.remaining_quantity_mt = Decimal("0")
        elif ask.remaining_quantity_mt < ask.quantity_mt:
            ask.status = OrderBookStatus.PARTIALLY_FILLED

    await db.flush()
    print(f"[market_seed] Created {trades_created} trades.")

    # ------------------------------------------------------------------
    # Step 4: Create RFQs with quotes
    # ------------------------------------------------------------------
    print("[market_seed] Creating RFQs...")
    rfqs_created = 0

    rfq_configs = [
        ("Methanol Green", "ARA",       2000, 845.00),
        ("Methanol Green", "Singapore", 3000, 890.00),
        ("VLSFO Conventional", "ARA",   5000, None),
        ("VLSFO Conventional", "Fujairah", 3500, 515.00),
        ("Biofuel Bio",    "ARA",       1500, 760.00),
        ("Biofuel Bio",    "Singapore", 2000, None),
        ("Ammonia Green",  "ARA",       2500, 620.00),
        ("LNG Conventional", "Singapore", 4000, 900.00),
        ("MGO Conventional", "ARA",     1000, None),
        ("MGO Conventional", "Singapore", 1500, 580.00),
    ]

    for i, (product_name, port_name, qty, target_price) in enumerate(rfq_configs):
        buyer = _RNG.choice(BUYER_ORGS)
        status = _RNG.choices(
            [RFQStatus.OPEN, RFQStatus.QUOTED, RFQStatus.ACCEPTED, RFQStatus.EXPIRED],
            weights=[30, 35, 20, 15],
        )[0]

        created = _rand_date(
            datetime(2025, 2, 1, tzinfo=timezone.utc),
            datetime(2025, 3, 18, tzinfo=timezone.utc),
        )
        expires = created + timedelta(days=_RNG.randint(3, 14))

        rfq = RFQ(
            id=uuid.uuid4(),
            buyer_org_id=buyer["id"],
            product_id=PRODUCT_IDS[product_name],
            delivery_point_id=DELIVERY_POINT_IDS[port_name],
            quantity_mt=Decimal(str(qty)),
            target_price_per_mt=Decimal(str(target_price)) if target_price else None,
            availability_window=_window(),
            notes=RFQ_NOTES[i],
            is_anonymous=_RNG.random() < 0.3,
            status=status,
            expires_at=expires,
            created_at=created,
        )
        db.add(rfq)
        await db.flush()  # so rfq.id is available for quotes

        # Add 1-3 quotes per RFQ (only if status >= QUOTED)
        if status in (RFQStatus.QUOTED, RFQStatus.ACCEPTED):
            n_quotes = _RNG.randint(1, 3)
            pricing_range = PRICING.get(product_name, {}).get(port_name)
            for q_idx in range(n_quotes):
                seller = _RNG.choice(SUPPLIER_ORGS)
                # Quote price near the ask range
                if pricing_range:
                    _, _, ask_lo, ask_hi = pricing_range
                    quote_price = _price(ask_lo, ask_hi)
                else:
                    quote_price = _price(500, 900)

                q_status = QuoteStatus.PENDING
                if status == RFQStatus.ACCEPTED and q_idx == 0:
                    q_status = QuoteStatus.ACCEPTED
                elif status == RFQStatus.ACCEPTED and q_idx > 0:
                    q_status = QuoteStatus.DECLINED

                quote = RFQQuote(
                    id=uuid.uuid4(),
                    rfq_id=rfq.id,
                    seller_org_id=seller["id"],
                    price_per_mt_usd=quote_price,
                    notes=_RNG.choice(QUOTE_NOTES),
                    status=q_status,
                    created_at=created + timedelta(hours=_RNG.randint(2, 72)),
                )
                db.add(quote)

        rfqs_created += 1

    await db.flush()
    print(f"[market_seed] Created {rfqs_created} RFQs.")

    # ------------------------------------------------------------------
    # Commit everything
    # ------------------------------------------------------------------
    await db.commit()
    print("[market_seed] Done. Market data seeded successfully.")

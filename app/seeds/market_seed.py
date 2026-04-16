"""Realistic market seed data — orders, trades, and RFQs.

Populates the orderbook with ~105 orders across all fuel-type/port combos,
~40 matched trades, and ~10 RFQs with quotes.  Prices reflect 2025-2026
marine fuel markets.

Idempotent: checks for a sentinel order before inserting.  Clears old test
data on first run.
"""
import random
import uuid
from datetime import date, datetime, timedelta, timezone
from decimal import Decimal

from sqlalchemy import text, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import (
    OrderBookOrder,
    Trade,
    OrderSide,
    OrderBookStatus,
    TradeStatus,
    Initiator,
)
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus
from app.seeds.catalog_seed import PRODUCT_IDS, DELIVERY_POINT_IDS
from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window
from app.services.execution_policy import normalize_certification_scheme

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
_SLICE_SCHEME_NS = uuid.UUID("c4d5e6f7-a8b9-4012-9abc-def123456789")

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
    "Bio Methanol": {
        "Singapore": (1020, 1070, 1090, 1140),
        "Shanghai": (980, 1035, 1050, 1105),
        "Dalian": (955, 1005, 1025, 1075),
        "Amsterdam": (540, 578, 595, 640),
        "Rotterdam": (545, 585, 600, 645),
        "Antwerp": (548, 588, 603, 648),
    },
    "e-Methanol": {
        "Singapore": (1090, 1140, 1160, 1210),
        "Shanghai": (1045, 1095, 1110, 1160),
        "Dalian": (1020, 1070, 1085, 1135),
        "Amsterdam": (610, 648, 665, 710),
        "Rotterdam": (615, 655, 670, 715),
        "Antwerp": (620, 660, 675, 720),
    },
    "Bio Ethanol": {
        "Singapore": (610, 655, 670, 715),
        "Shanghai": (590, 635, 650, 695),
        "Dalian": (575, 620, 635, 680),
        "Amsterdam": (558, 598, 613, 653),
        "Rotterdam": (565, 605, 620, 660),
        "Antwerp": (568, 608, 623, 663),
    },
    "Synthetic Ethanol": {
        "Singapore": (700, 745, 760, 810),
        "Shanghai": (680, 725, 740, 790),
        "Dalian": (665, 710, 725, 775),
        "Amsterdam": (648, 688, 703, 743),
        "Rotterdam": (655, 695, 710, 750),
        "Antwerp": (660, 700, 715, 755),
    },
}

# CI data ranges per product: (ci_lo, ci_hi, energy_density)
CI_DATA: dict[str, tuple[float, float, float]] = {
    "Bio Ethanol":        (8, 18, 26.8),
    "Bio Methanol":       (25, 55, 19.9),
    "e-Methanol":         (5, 20, 19.9),
    "Synthetic Ethanol":  (15, 35, 26.8),
}

CERTIFICATION_SCHEMES = ("ISCC EU", "ISCC PLUS", "REDcert EU")

DEMO_BUYER_ORG_ID = uuid.UUID("acc3f20a-fe94-4463-9029-a55e35634eb7")
DEMO_SELLER_ORG_ID = uuid.UUID("c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4")

DEMO_SLICE_DEPTH_BIDS = [
    (0, Decimal("5000"), Decimal("5000"), Decimal("1048.00"), OrderBookStatus.OPEN),
    (1, Decimal("3500"), Decimal("3500"), Decimal("1045.00"), OrderBookStatus.OPEN),
    (2, Decimal("2500"), Decimal("2000"), Decimal("1041.00"), OrderBookStatus.PARTIALLY_FILLED),
    (3, Decimal("2000"), Decimal("2000"), Decimal("1036.00"), OrderBookStatus.OPEN),
    (4, Decimal("1500"), Decimal("1000"), Decimal("1032.00"), OrderBookStatus.PARTIALLY_FILLED),
]

DEMO_SLICE_DEPTH_ASKS = [
    (0, Decimal("4000"), Decimal("4000"), Decimal("1056.00"), OrderBookStatus.OPEN),
    (1, Decimal("3000"), Decimal("3000"), Decimal("1061.00"), OrderBookStatus.OPEN),
    (2, Decimal("2500"), Decimal("1800"), Decimal("1067.00"), OrderBookStatus.PARTIALLY_FILLED),
    (3, Decimal("2000"), Decimal("2000"), Decimal("1074.00"), OrderBookStatus.OPEN),
    (4, Decimal("1500"), Decimal("1200"), Decimal("1082.00"), OrderBookStatus.PARTIALLY_FILLED),
]

DEMO_ACCOUNT_TRADE_CONFIGS = [
    ("Bio Methanol", "Singapore", SPOT_WINDOW, Decimal("500"), Decimal("1056.00"), TradeStatus.PENDING_CONFIRMATION, Initiator.BUYER, 1),
    ("Bio Methanol", "Rotterdam", SPOT_WINDOW, Decimal("750"), Decimal("602.00"), TradeStatus.PENDING_CONFIRMATION, Initiator.SELLER, 2),
    ("e-Methanol", "Singapore", "2026-06", Decimal("1500"), Decimal("1184.00"), TradeStatus.CONFIRMED, Initiator.BUYER, 4),
    ("Bio Ethanol", "Shanghai", "2026-05", Decimal("1200"), Decimal("668.00"), TradeStatus.CONFIRMED, Initiator.SELLER, 5),
    ("Synthetic Ethanol", "Amsterdam", "2026-05", Decimal("900"), Decimal("709.50"), TradeStatus.DELIVERED, Initiator.SELLER, 8),
    ("Bio Methanol", "Antwerp", "2026-Q3", Decimal("1800"), Decimal("608.00"), TradeStatus.PAID, Initiator.BUYER, 15),
]

def build_seed_windows(reference_date: date | None = None, *, quarter_count: int = 6) -> list[str]:
    current = reference_date or date.today()
    current_quarter = ((current.month - 1) // 3) + 1
    current_quarter_end_month = current_quarter * 3

    windows = [SPOT_WINDOW]
    for month in range(current.month, current_quarter_end_month + 1):
        windows.append(f"{current.year}-{month:02d}")

    quarter_year = current.year
    quarter = current_quarter + 1
    for _ in range(quarter_count):
        if quarter > 4:
            quarter = 1
            quarter_year += 1
        windows.append(f"{quarter_year}-Q{quarter}")
        quarter += 1

    return windows


WINDOWS = build_seed_windows()

QUANTITIES = [500, 1000, 1500, 2000, 2500, 3000, 3500, 4000, 4500, 5000]

# RFQ notes templates
RFQ_NOTES = [
    "Seeking competitive quotes for upcoming voyage refueling",
    "Annual contract renewal — need firm offers for next quarter",
    "Spot requirement, flexible on post-match scheduling details",
    "Looking for ISCC-certified supply only",
    "Fleet-wide procurement, multiple deliveries expected",
    "Urgent requirement — vessel arriving next week",
    "Testing new fuel pathway for FuelEU compliance",
    "Require full chain-of-custody documentation",
    "Prefer suppliers with Amsterdam/Rotterdam/Antwerp delivery capability",
    "Need blending options — open to partial bio blends",
]

QUOTE_NOTES = [
    "Ex-tank Rotterdam, loading within 3 days of confirmation",
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


def _window() -> str:
    return _RNG.choice(WINDOWS)


def _rand_date(start: datetime, end: datetime) -> datetime:
    delta = end - start
    offset = _RNG.randint(0, int(delta.total_seconds()))
    return start + timedelta(seconds=offset)


def _slice_certification_scheme(product_name: str, port_name: str, window: str) -> str:
    normalized_window = normalize_availability_window(window)
    index = uuid.uuid5(_SLICE_SCHEME_NS, f"{product_name}|{port_name}|{normalized_window}").int % len(CERTIFICATION_SCHEMES)
    return CERTIFICATION_SCHEMES[index]


def _seed_price_for_slice(
    side: OrderSide,
    *,
    bid_lo: float,
    bid_hi: float,
    ask_lo: float,
    ask_hi: float,
    window: str,
    depth_index: int = 0,
) -> Decimal:
    normalized_window = normalize_availability_window(window)
    premium = _window_premium(normalized_window, ask_lo=ask_lo, ask_hi=ask_hi)
    best_bid = Decimal(str(round((bid_lo + bid_hi) / 2, 2))) + premium
    natural_ask = Decimal(str(round((ask_lo + ask_hi) / 2, 2))) + premium
    spread_floor = Decimal(str(max(round((ask_lo - bid_hi) * 0.6, 2), 12.0)))
    ladder_step = Decimal(str(max(round((ask_hi - ask_lo) * 0.18, 2), 4.0)))
    best_ask = max(natural_ask, best_bid + spread_floor)

    price = best_bid - (ladder_step * depth_index) if side == OrderSide.BID else best_ask + (ladder_step * depth_index)
    return price.quantize(Decimal('0.01'))


def _recent_seed_timestamp(window: str, reference_now: datetime, *, trade: bool = False) -> datetime:
    normalized_window = normalize_availability_window(window)
    if normalized_window == SPOT_WINDOW:
        min_days, max_days = (0, 2) if not trade else (0, 1)
    elif len(normalized_window) == 7 and normalized_window[4] == '-':
        min_days, max_days = (1, 5) if not trade else (1, 3)
    else:
        min_days, max_days = (3, 10) if not trade else (2, 6)

    start = reference_now - timedelta(days=max_days, hours=12)
    end = reference_now - timedelta(days=min_days)
    if trade and min_days == 0:
        end = reference_now - timedelta(minutes=15)
    if end <= start:
        end = start + timedelta(hours=1)
    return _rand_date(start, end)


def _clamp_trade_timeline(
    created: datetime,
    confirmed: datetime | None,
    delivered: datetime | None,
    paid: datetime | None,
    reference_now: datetime,
) -> tuple[datetime, datetime | None, datetime | None, datetime | None]:
    confirmed = min(confirmed, reference_now) if confirmed else None
    delivered = min(delivered, reference_now) if delivered else None
    paid = min(paid, reference_now) if paid else None

    if delivered and confirmed and delivered < confirmed:
        delivered = confirmed
    if paid and delivered and paid < delivered:
        paid = delivered
    elif paid and confirmed and paid < confirmed:
        paid = confirmed

    if confirmed and created > confirmed:
        created = confirmed
    elif created > reference_now:
        created = reference_now

    return created, confirmed, delivered, paid


def bid_seed_metadata(certification_scheme: str | None = None) -> dict[str, str]:
    return {
        "certification_scheme": certification_scheme or _RNG.choice(CERTIFICATION_SCHEMES),
    }


def ask_seed_metadata(certification_scheme: str | None = None) -> dict[str, object]:
    certification_scheme = certification_scheme or _RNG.choice(CERTIFICATION_SCHEMES)
    return {
        "certification_declared": True,
        "certification_scheme": certification_scheme,
        "certifications": [certification_scheme],
        "specification_standard": _RNG.choice(["IMPCA", "ASTM D4806", "Supplier COA"]),
        "msds_available": True,
        "feedstock": _RNG.choice(["Waste residue", "Agricultural residue", "Biogenic CO2 + green hydrogen"]),
        "carbon_intensity_method": _RNG.choice(["ISCC lifecycle", "Producer LCA", "FuelEU dossier"]),
    }


def _window_premium(window: str, *, ask_lo: float, ask_hi: float) -> Decimal:
    if window == SPOT_WINDOW:
        return Decimal("0")

    try:
        index = WINDOWS.index(window)
    except ValueError:
        return Decimal("0")

    spread = ask_hi - ask_lo
    step = max(index - 1, 0)
    if len(window) == 7 and window[4] == "-":
        premium = spread * (0.12 + (0.08 * step))
    else:
        premium = spread * (0.35 + (0.1 * step))
    premium = round(premium, 2)
    return Decimal(str(premium))


def _orders_share_executable_slice(bid: OrderBookOrder, ask: OrderBookOrder) -> bool:
    return (
        bid.product_id == ask.product_id
        and bid.delivery_point_id == ask.delivery_point_id
        and normalize_availability_window(bid.availability_window) == normalize_availability_window(ask.availability_window)
        and normalize_certification_scheme(bid.certification_scheme) == normalize_certification_scheme(ask.certification_scheme)
    )


def _demo_trade_timestamps(
    *,
    reference_now: datetime,
    status: TradeStatus,
    days_ago: int,
) -> tuple[datetime, datetime | None, datetime | None, datetime | None]:
    created = reference_now - timedelta(days=days_ago, hours=3)
    confirmed = created + timedelta(hours=6) if status in (TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID) else None
    delivered = confirmed + timedelta(days=2) if confirmed and status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None
    paid = delivered + timedelta(days=3) if delivered and status == TradeStatus.PAID else None
    return _clamp_trade_timeline(created, confirmed, delivered, paid, reference_now)

# ---------------------------------------------------------------------------
# Core seed function
# ---------------------------------------------------------------------------

async def seed_market_data(db: AsyncSession, *, force_reset: bool = False) -> None:
    """Seed realistic market data. Idempotent unless force_reset is requested."""

    # Check sentinel
    existing = (await db.execute(
        select(OrderBookOrder.id).where(OrderBookOrder.id == _SENTINEL_ID)
    )).scalar_one_or_none()

    if existing is not None and not force_reset:
        print("[market_seed] Sentinel found — already seeded, skipping.")
        return

    if existing is not None and force_reset:
        print("[market_seed] Sentinel found — force reset requested, reseeding market data.")

    # ------------------------------------------------------------------
    # Step 0: Clear old test data (reverse FK order)
    # ------------------------------------------------------------------
    print("[market_seed] Clearing old test data...")
    await db.execute(text("DELETE FROM watchlist_events WHERE watchlist_target_id IN (SELECT id FROM watchlist_targets WHERE order_id IS NOT NULL)"))
    await db.execute(text("DELETE FROM watchlist_targets WHERE order_id IS NOT NULL"))
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

    reference_now = datetime.now(timezone.utc)

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
                window = _window()
                certification_scheme = _slice_certification_scheme(product_name, port_name, window)
                bid_metadata = bid_seed_metadata(certification_scheme)
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

                created = _recent_seed_timestamp(window, reference_now)

                order = OrderBookOrder(
                    id=uuid.uuid4(),
                    organization_id=buyer["id"],
                    side=OrderSide.BID,
                    product_id=product_id,
                    delivery_point_id=dp_id,
                    quantity_mt=qty,
                    remaining_quantity_mt=remaining,
                    price_per_mt_usd=_seed_price_for_slice(OrderSide.BID, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi, window=window, depth_index=_RNG.randint(0, 2)),
                    availability_window=window,
                    certification_scheme=bid_metadata["certification_scheme"],
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

                window = _window()
                certification_scheme = _slice_certification_scheme(product_name, port_name, window)
                ci_value = Decimal(str(round(_RNG.uniform(ci_lo, ci_hi), 2)))
                ask_metadata = ask_seed_metadata(certification_scheme)

                created = _recent_seed_timestamp(window, reference_now)

                order = OrderBookOrder(
                    id=uuid.uuid4(),
                    organization_id=supplier["id"],
                    side=OrderSide.ASK,
                    product_id=product_id,
                    delivery_point_id=dp_id,
                    quantity_mt=qty,
                    remaining_quantity_mt=remaining,
                    price_per_mt_usd=_seed_price_for_slice(OrderSide.ASK, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi, window=window, depth_index=_RNG.randint(0, 2)),
                    availability_window=window,
                    status=status,
                    certifications=ask_metadata["certifications"],
                    certification_declared=ask_metadata["certification_declared"],
                    certification_scheme=ask_metadata["certification_scheme"],
                    specification_standard=ask_metadata["specification_standard"],
                    msds_available=ask_metadata["msds_available"],
                    carbon_intensity_method=ask_metadata["carbon_intensity_method"],
                    feedstock=ask_metadata["feedstock"],
                    origin=f"{port_name} hub",
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
        product_id=PRODUCT_IDS["Bio Methanol"],
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
        quantity_mt=Decimal("0"),
        remaining_quantity_mt=Decimal("0"),
        price_per_mt_usd=Decimal("0"),
        availability_window=SPOT_WINDOW,
        status=OrderBookStatus.CANCELLED,
        created_at=datetime(2020, 1, 1, tzinfo=timezone.utc),
        updated_at=datetime(2020, 1, 1, tzinfo=timezone.utc),
    )
    db.add(sentinel)
    await db.flush()

    total_orders = len(all_orders)
    print(f"[market_seed] Created {total_orders} orders + sentinel.")

    # ------------------------------------------------------------------
    # Step 2b: Coverage guarantee — ensure every window has orders
    # ------------------------------------------------------------------
    print("[market_seed] Running coverage guarantee pass...")
    coverage_created = 0
    for product_name, ports in PRICING.items():
        product_id = PRODUCT_IDS[product_name]
        ci_lo, ci_hi, energy_density = CI_DATA[product_name]

        for port_name, (bid_lo, bid_hi, ask_lo, ask_hi) in ports.items():
            dp_id = DELIVERY_POINT_IDS[port_name]
            key = f"{product_name}|{port_name}"
            existing_orders = orders_by_product_port.get(key, [])

            active_orders = [
                o for o in existing_orders
                if o.status in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
            ]

            for window in WINDOWS:
                anchor_bid_price = _seed_price_for_slice(
                    OrderSide.BID,
                    bid_lo=bid_lo,
                    bid_hi=bid_hi,
                    ask_lo=ask_lo,
                    ask_hi=ask_hi,
                    window=window,
                    depth_index=0,
                )
                anchor_ask_price = _seed_price_for_slice(
                    OrderSide.ASK,
                    bid_lo=bid_lo,
                    bid_hi=bid_hi,
                    ask_lo=ask_lo,
                    ask_hi=ask_hi,
                    window=window,
                    depth_index=0,
                )
                needs_bid = not any(
                    o.side == OrderSide.BID
                    and normalize_availability_window(o.availability_window) == normalize_availability_window(window)
                    and o.price_per_mt_usd >= anchor_bid_price
                    for o in active_orders
                )
                needs_ask = not any(
                    o.side == OrderSide.ASK
                    and normalize_availability_window(o.availability_window) == normalize_availability_window(window)
                    and o.price_per_mt_usd <= anchor_ask_price
                    for o in active_orders
                )

                if not needs_bid and not needs_ask:
                    continue

                created = _recent_seed_timestamp(window, reference_now)

                if needs_bid:
                    buyer = _RNG.choice(BUYER_ORGS)
                    certification_scheme = _slice_certification_scheme(product_name, port_name, window)
                    bid_metadata = bid_seed_metadata(certification_scheme)
                    gap_qty = _qty()
                    order = OrderBookOrder(
                        id=uuid.uuid4(),
                        organization_id=buyer["id"],
                        side=OrderSide.BID,
                        product_id=product_id,
                        delivery_point_id=dp_id,
                        quantity_mt=gap_qty,
                        remaining_quantity_mt=gap_qty,
                        price_per_mt_usd=_seed_price_for_slice(OrderSide.BID, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi, window=window, depth_index=0),
                        availability_window=window,
                        certification_scheme=bid_metadata["certification_scheme"],
                        status=OrderBookStatus.OPEN,
                        created_at=created,
                        updated_at=created,
                    )
                    db.add(order)
                    all_orders.append(order)
                    orders_by_product_port.setdefault(key, []).append(order)
                    coverage_created += 1

                if needs_ask:
                    supplier = _RNG.choice(SUPPLIER_ORGS)
                    ci_value = Decimal(str(round(_RNG.uniform(ci_lo, ci_hi), 2)))
                    gap_qty = _qty()
                    certification_scheme = _slice_certification_scheme(product_name, port_name, window)
                    ask_metadata = ask_seed_metadata(certification_scheme)
                    order = OrderBookOrder(
                        id=uuid.uuid4(),
                        organization_id=supplier["id"],
                        side=OrderSide.ASK,
                        product_id=product_id,
                        delivery_point_id=dp_id,
                        quantity_mt=gap_qty,
                        remaining_quantity_mt=gap_qty,
                        price_per_mt_usd=_seed_price_for_slice(OrderSide.ASK, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi, window=window, depth_index=0),
                        availability_window=window,
                        status=OrderBookStatus.OPEN,
                        certification_declared=ask_metadata["certification_declared"],
                        certification_scheme=ask_metadata["certification_scheme"],
                        certifications=ask_metadata["certifications"],
                        specification_standard=ask_metadata["specification_standard"],
                        msds_available=ask_metadata["msds_available"],
                        carbon_intensity_method=ask_metadata["carbon_intensity_method"],
                        feedstock=ask_metadata["feedstock"],
                        origin=f"{port_name} hub",
                        carbon_intensity_gco2_mj=ci_value,
                        energy_density_mj_kg=Decimal(str(energy_density)),
                        created_at=created,
                        updated_at=created,
                    )
                    db.add(order)
                    all_orders.append(order)
                    orders_by_product_port.setdefault(key, []).append(order)
                    coverage_created += 1

    await db.flush()
    print(f"[market_seed] Coverage pass: created {coverage_created} gap-filling orders.")

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
                if not _orders_share_executable_slice(bid, ask):
                    continue
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

        created = _recent_seed_timestamp(ask.availability_window, reference_now, trade=True)
        confirmed = created + timedelta(hours=_RNG.randint(1, 48))
        delivered = confirmed + timedelta(days=_RNG.randint(3, 21)) if status in (
            TradeStatus.DELIVERED, TradeStatus.PAID
        ) else None
        paid = delivered + timedelta(days=_RNG.randint(7, 30)) if status == TradeStatus.PAID and delivered else None
        created, confirmed, delivered, paid = _clamp_trade_timeline(
            created,
            confirmed,
            delivered,
            paid,
            reference_now,
        )

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
    # Step 3b: Post-trade coverage safety net
    # Some gap-fill orders may have been consumed by trades above.
    # Re-check and fill any windows that lost all active orders.
    # ------------------------------------------------------------------
    print("[market_seed] Post-trade coverage check...")
    post_trade_created = 0
    for product_name, ports in PRICING.items():
        product_id = PRODUCT_IDS[product_name]
        ci_lo, ci_hi, energy_density = CI_DATA[product_name]

        for port_name, (bid_lo, bid_hi, ask_lo, ask_hi) in ports.items():
            dp_id = DELIVERY_POINT_IDS[port_name]
            key = f"{product_name}|{port_name}"
            existing_orders = orders_by_product_port.get(key, [])

            active_orders = [
                o for o in existing_orders
                if o.status in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
            ]

            for window in WINDOWS:
                created = _recent_seed_timestamp(window, reference_now)
                anchor_bid_price = _seed_price_for_slice(
                    OrderSide.BID,
                    bid_lo=bid_lo,
                    bid_hi=bid_hi,
                    ask_lo=ask_lo,
                    ask_hi=ask_hi,
                    window=window,
                    depth_index=0,
                )
                anchor_ask_price = _seed_price_for_slice(
                    OrderSide.ASK,
                    bid_lo=bid_lo,
                    bid_hi=bid_hi,
                    ask_lo=ask_lo,
                    ask_hi=ask_hi,
                    window=window,
                    depth_index=0,
                )
                needs_bid = not any(
                    o.side == OrderSide.BID
                    and normalize_availability_window(o.availability_window) == normalize_availability_window(window)
                    and o.price_per_mt_usd >= anchor_bid_price
                    for o in active_orders
                )
                needs_ask = not any(
                    o.side == OrderSide.ASK
                    and normalize_availability_window(o.availability_window) == normalize_availability_window(window)
                    and o.price_per_mt_usd <= anchor_ask_price
                    for o in active_orders
                )

                if needs_bid:
                    buyer = _RNG.choice(BUYER_ORGS)
                    certification_scheme = _slice_certification_scheme(product_name, port_name, window)
                    bid_metadata = bid_seed_metadata(certification_scheme)
                    gap_qty = _qty()
                    order = OrderBookOrder(
                        id=uuid.uuid4(),
                        organization_id=buyer["id"],
                        side=OrderSide.BID,
                        product_id=product_id,
                        delivery_point_id=dp_id,
                        quantity_mt=gap_qty,
                        remaining_quantity_mt=gap_qty,
                        price_per_mt_usd=_seed_price_for_slice(OrderSide.BID, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi, window=window, depth_index=0),
                        availability_window=window,
                        certification_scheme=bid_metadata["certification_scheme"],
                        status=OrderBookStatus.OPEN,
                        created_at=created,
                        updated_at=created,
                    )
                    db.add(order)
                    post_trade_created += 1

                if needs_ask:
                    supplier = _RNG.choice(SUPPLIER_ORGS)
                    ci_value = Decimal(str(round(_RNG.uniform(ci_lo, ci_hi), 2)))
                    gap_qty = _qty()
                    certification_scheme = _slice_certification_scheme(product_name, port_name, window)
                    ask_metadata = ask_seed_metadata(certification_scheme)
                    order = OrderBookOrder(
                        id=uuid.uuid4(),
                        organization_id=supplier["id"],
                        side=OrderSide.ASK,
                        product_id=product_id,
                        delivery_point_id=dp_id,
                        quantity_mt=gap_qty,
                        remaining_quantity_mt=gap_qty,
                        price_per_mt_usd=_seed_price_for_slice(OrderSide.ASK, bid_lo=bid_lo, bid_hi=bid_hi, ask_lo=ask_lo, ask_hi=ask_hi, window=window, depth_index=0),
                        availability_window=window,
                        status=OrderBookStatus.OPEN,
                        certifications=ask_metadata["certifications"],
                        certification_declared=ask_metadata["certification_declared"],
                        certification_scheme=ask_metadata["certification_scheme"],
                        specification_standard=ask_metadata["specification_standard"],
                        msds_available=ask_metadata["msds_available"],
                        carbon_intensity_gco2_mj=Decimal(str(round(_RNG.uniform(ci_lo, ci_hi), 2))),
                        carbon_intensity_method=ask_metadata["carbon_intensity_method"],
                        feedstock=ask_metadata["feedstock"],
                        origin=f"{port_name} hub",
                        energy_density_mj_kg=Decimal(str(energy_density)),
                        created_at=created,
                        updated_at=created,
                    )
                    db.add(order)
                    post_trade_created += 1

    if post_trade_created > 0:
        await db.flush()
    print(f"[market_seed] Post-trade safety net: created {post_trade_created} orders.")

    # ------------------------------------------------------------------
    # Step 4: Create RFQs with quotes
    # ------------------------------------------------------------------
    print("[market_seed] Creating RFQs...")
    rfqs_created = 0

    rfq_configs = [
        ("Bio Methanol",      "Amsterdam", 2000, 575.00),
        ("Bio Methanol",      "Singapore", 3000, 1065.00),
        ("Bio Ethanol",       "Antwerp",   2500, 630.00),
        ("Bio Ethanol",       "Shanghai",  1800, None),
        ("Synthetic Ethanol", "Rotterdam", 1500, 720.00),
        ("Synthetic Ethanol", "Singapore", 2000, None),
        ("e-Methanol",        "Amsterdam", 2500, 650.00),
        ("e-Methanol",        "Rotterdam", 1000, None),
        ("e-Methanol",        "Dalian",    1500, 890.00),
        ("Synthetic Ethanol", "Antwerp",   2000, 720.00),
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
    # Step 4b: Add explicit demo depth for Bio Methanol / Singapore / Spot
    # using only non-demo organizations so the recording accounts stay clean.
    # ------------------------------------------------------------------
    print("[market_seed] Creating explicit Bio Methanol / Singapore / Spot depth...")

    demo_depth_created = 0
    demo_product_name = "Bio Methanol"
    demo_port_name = "Singapore"
    demo_product_id = PRODUCT_IDS[demo_product_name]
    demo_delivery_point_id = DELIVERY_POINT_IDS[demo_port_name]
    demo_certification_scheme = _slice_certification_scheme(demo_product_name, demo_port_name, SPOT_WINDOW)
    demo_ci_lo, demo_ci_hi, demo_energy_density = CI_DATA[demo_product_name]

    for org_index, quantity_mt, remaining_quantity_mt, price_per_mt_usd, status in DEMO_SLICE_DEPTH_BIDS:
        created = reference_now - timedelta(hours=12 + demo_depth_created)
        bid_metadata = bid_seed_metadata(demo_certification_scheme)
        order = OrderBookOrder(
            id=uuid.uuid4(),
            organization_id=BUYER_ORGS[org_index % len(BUYER_ORGS)]["id"],
            side=OrderSide.BID,
            product_id=demo_product_id,
            delivery_point_id=demo_delivery_point_id,
            quantity_mt=quantity_mt,
            remaining_quantity_mt=remaining_quantity_mt,
            price_per_mt_usd=price_per_mt_usd,
            availability_window=SPOT_WINDOW,
            certification_scheme=bid_metadata["certification_scheme"],
            status=status,
            created_at=created,
            updated_at=created,
        )
        db.add(order)
        demo_depth_created += 1

    for org_index, quantity_mt, remaining_quantity_mt, price_per_mt_usd, status in DEMO_SLICE_DEPTH_ASKS:
        created = reference_now - timedelta(hours=12 + demo_depth_created)
        ask_metadata = ask_seed_metadata(demo_certification_scheme)
        order = OrderBookOrder(
            id=uuid.uuid4(),
            organization_id=SUPPLIER_ORGS[org_index % len(SUPPLIER_ORGS)]["id"],
            side=OrderSide.ASK,
            product_id=demo_product_id,
            delivery_point_id=demo_delivery_point_id,
            quantity_mt=quantity_mt,
            remaining_quantity_mt=remaining_quantity_mt,
            price_per_mt_usd=price_per_mt_usd,
            availability_window=SPOT_WINDOW,
            status=status,
            certifications=ask_metadata["certifications"],
            certification_declared=ask_metadata["certification_declared"],
            certification_scheme=ask_metadata["certification_scheme"],
            specification_standard=ask_metadata["specification_standard"],
            msds_available=ask_metadata["msds_available"],
            carbon_intensity_method=ask_metadata["carbon_intensity_method"],
            feedstock=ask_metadata["feedstock"],
            origin=f"{demo_port_name} hub",
            is_verdaxis_verified=True,
            carbon_intensity_gco2_mj=Decimal(str(round(_RNG.uniform(demo_ci_lo, demo_ci_hi), 2))),
            energy_density_mj_kg=Decimal(str(demo_energy_density)),
            created_at=created,
            updated_at=created,
        )
        db.add(order)
        demo_depth_created += 1

    await db.flush()
    print(f"[market_seed] Created {demo_depth_created} explicit depth orders for the demo slice.")

    # ------------------------------------------------------------------
    # Step 5: Create resettable demo-account trades across lifecycle states.
    # Each trade gets linked filled BID/ASK orders so Trade History shows the
    # product, port, and window context without leaking demo liquidity onto the
    # public book.
    # ------------------------------------------------------------------
    print("[market_seed] Creating Buy Corp / Sell Corp demo trades...")

    await db.execute(text(
        "INSERT INTO organizations (id, name, type, verification_status) "
        "VALUES (:id, :name, 'SHIPPING_LINE', 'APPROVED') "
        "ON CONFLICT (id) DO NOTHING"
    ), {"id": DEMO_BUYER_ORG_ID, "name": "Buy Corp"})
    await db.execute(text(
        "INSERT INTO organizations (id, name, type, supplier_tier, verification_status) "
        "VALUES (:id, :name, 'FUEL_SUPPLIER', 'REGIONAL_SUPPLIER', 'APPROVED') "
        "ON CONFLICT (id) DO NOTHING"
    ), {"id": DEMO_SELLER_ORG_ID, "name": "Sell Corp"})
    await db.flush()

    demo_trades_created = 0
    for fuel_name, port_name, availability_window, trade_qty, trade_price, status, initiator, days_ago in DEMO_ACCOUNT_TRADE_CONFIGS:
        product_id = PRODUCT_IDS[fuel_name]
        dp_id = DELIVERY_POINT_IDS[port_name]
        certification_scheme = _slice_certification_scheme(fuel_name, port_name, availability_window)
        ci_lo, ci_hi, energy_density = CI_DATA[fuel_name]
        bid_created_at, confirmed_at, delivered_at, paid_at = _demo_trade_timestamps(
            reference_now=reference_now,
            status=status,
            days_ago=days_ago,
        )
        order_created_at = bid_created_at - timedelta(hours=2)

        ask_metadata = ask_seed_metadata(certification_scheme)
        bid_order = OrderBookOrder(
            id=uuid.uuid4(),
            organization_id=DEMO_BUYER_ORG_ID,
            side=OrderSide.BID,
            product_id=product_id,
            delivery_point_id=dp_id,
            quantity_mt=trade_qty,
            remaining_quantity_mt=Decimal("0"),
            price_per_mt_usd=trade_price,
            availability_window=availability_window,
            certification_scheme=certification_scheme,
            status=OrderBookStatus.FILLED,
            created_at=order_created_at,
            updated_at=bid_created_at,
        )
        ask_order = OrderBookOrder(
            id=uuid.uuid4(),
            organization_id=DEMO_SELLER_ORG_ID,
            side=OrderSide.ASK,
            product_id=product_id,
            delivery_point_id=dp_id,
            quantity_mt=trade_qty,
            remaining_quantity_mt=Decimal("0"),
            price_per_mt_usd=trade_price,
            availability_window=availability_window,
            status=OrderBookStatus.FILLED,
            certifications=ask_metadata["certifications"],
            certification_declared=ask_metadata["certification_declared"],
            certification_scheme=ask_metadata["certification_scheme"],
            specification_standard=ask_metadata["specification_standard"],
            msds_available=ask_metadata["msds_available"],
            carbon_intensity_method=ask_metadata["carbon_intensity_method"],
            feedstock=ask_metadata["feedstock"],
            origin=f"{port_name} hub",
            is_verdaxis_verified=True,
            carbon_intensity_gco2_mj=Decimal(str(round(_RNG.uniform(ci_lo, ci_hi), 2))),
            energy_density_mj_kg=Decimal(str(energy_density)),
            created_at=order_created_at,
            updated_at=bid_created_at,
        )
        db.add(bid_order)
        db.add(ask_order)
        await db.flush()

        total_usd = trade_qty * trade_price
        commission_rate = Decimal("0.500")
        commission_amt = (total_usd * commission_rate / Decimal("100")).quantize(Decimal("0.01"))

        trade = Trade(
            id=uuid.uuid4(),
            bid_order_id=bid_order.id,
            ask_order_id=ask_order.id,
            buyer_id=DEMO_BUYER_ORG_ID,
            seller_id=DEMO_SELLER_ORG_ID,
            initiated_by=initiator,
            quantity_mt=trade_qty,
            price_per_mt_usd=trade_price,
            status=status,
            final_quantity_mt=trade_qty if status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None,
            final_price_per_mt=trade_price if status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None,
            final_total_usd=total_usd if status in (TradeStatus.DELIVERED, TradeStatus.PAID) else None,
            commission_rate_pct=commission_rate,
            commission_amount_usd=commission_amt if status == TradeStatus.PAID else None,
            confirmed_at=confirmed_at,
            delivered_at=delivered_at,
            paid_at=paid_at,
            created_at=bid_created_at,
        )
        db.add(trade)
        demo_trades_created += 1

    await db.flush()
    print(f"[market_seed] Created {demo_trades_created} Buy Corp / Sell Corp demo trades.")

    # ------------------------------------------------------------------
    # Commit everything
    # ------------------------------------------------------------------
    await db.commit()
    print("[market_seed] Done. Market data seeded successfully.")

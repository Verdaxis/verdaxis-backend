"""Realistic market seed data — orders, trades, and RFQs.

Populates the orderbook with ~105 orders across all fuel-type/port combos,
~40 matched trades, and ~10 RFQs with quotes.  Prices reflect 2025-2026
marine fuel markets.

Idempotent: checks for a metadata seed-run marker before inserting. Clears
only the explicitly authorized synthetic fixture on reset.
"""
import random
import uuid
import os
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
from app.demo_identities import (
    DEMO_ACCOUNT_BUYER_ORG_ID,
    DEMO_ACCOUNT_SELLER_ORG_ID,
    DEMO_SEED_BUYERS,
    DEMO_SEED_SUPPLIERS,
)
from app.seeds.catalog_seed import PRODUCT_IDS, DELIVERY_POINT_IDS
from app.services.availability_windows import (
    SPOT_WINDOW,
    availability_window_expiry,
    normalize_availability_window,
    tradable_availability_windows,
)
from app.services.execution_policy import normalize_certification_scheme
from app.models.seed import SeedRun
from app.environment_database import validate_database_target

# ---------------------------------------------------------------------------
# Deterministic seed for reproducibility
# ---------------------------------------------------------------------------
_RNG = random.Random(42)

# Idempotency is metadata, never an economic row.
MARKET_SEED_NAME = "market"
_SENTINEL_ID = None


def _seed_order(**values):
    """Create a demo seed order with an explicit immutable snapshot."""
    values.setdefault("provenance", "DEMO")
    observed_at = values.get("created_at") or datetime.now(timezone.utc)
    values.setdefault(
        "expires_at",
        availability_window_expiry(
            values.get("availability_window", SPOT_WINDOW),
            observed_at=observed_at,
        ),
    )
    return OrderBookOrder(**values)


def _seed_trade(**values):
    """Create a demo seed trade with immutable party snapshots."""
    values.setdefault("buyer_provenance", "DEMO")
    values.setdefault("seller_provenance", "DEMO")
    values.setdefault(
        "initiator_org_id",
        values["buyer_id"]
        if values.get("initiated_by") == Initiator.BUYER
        else values["seller_id"],
    )
    return Trade(**values)


def validate_demo_reset(
    *,
    environment: str,
    explicit_opt_in: bool,
    database_url: str,
    current_database: str,
) -> None:
    if environment.lower() == "production":
        raise RuntimeError("demo reset is categorically forbidden in production")
    if not explicit_opt_in:
        raise RuntimeError("demo reset requires explicit opt-in")
    try:
        validate_database_target(
            environment=environment,
            database_url=database_url,
            current_database=current_database,
        )
    except ValueError as exc:
        raise RuntimeError(f"demo reset database attestation failed: {exc}") from exc


def _demo_org_ids() -> tuple[str, ...]:
    return tuple(str(org["id"]) for org in (*BUYER_ORGS, *SUPPLIER_ORGS)) + (
        str(DEMO_BUYER_ORG_ID),
        str(DEMO_SELLER_ORG_ID),
        "7cc77115-0a9f-4ec4-8c74-05aa10050111",
        "d1e43e55-3fb0-4b5e-9f0b-93aa10050222",
    )


async def reset_demo_market_data(db: AsyncSession) -> None:
    """Delete only known synthetic/provenance-scoped rows in FK order."""
    ids = ",".join(f"'{value}'" for value in _demo_org_ids())
    demo_orders = f"SELECT id FROM orderbook_orders WHERE provenance = 'DEMO' AND organization_id IN ({ids})"
    demo_trades = (
        "SELECT id FROM trades WHERE buyer_id IN (" + ids + ") "
        "AND seller_id IN (" + ids + ") "
        "AND buyer_provenance = 'DEMO' AND seller_provenance = 'DEMO'"
    )
    await db.execute(text(f"DELETE FROM watchlist_events WHERE watchlist_target_id IN (SELECT id FROM watchlist_targets WHERE order_id IN ({demo_orders}))"))
    await db.execute(text(f"DELETE FROM watchlist_targets WHERE order_id IN ({demo_orders})"))
    await db.execute(text(f"DELETE FROM match_suggestions WHERE bid_order_id IN ({demo_orders}) OR ask_order_id IN ({demo_orders}) OR recipient_org_id IN ({ids})"))
    await db.execute(text(f"DELETE FROM commissions WHERE trade_id IN ({demo_trades})"))
    await db.execute(text(f"DELETE FROM trades WHERE id IN ({demo_trades})"))
    await db.execute(text(f"DELETE FROM rfq_quotes WHERE seller_org_id IN ({ids}) OR rfq_id IN (SELECT id FROM rfqs WHERE buyer_org_id IN ({ids}))"))
    await db.execute(text(f"DELETE FROM rfqs WHERE buyer_org_id IN ({ids})"))
    await db.execute(text(f"DELETE FROM negotiation_rounds WHERE negotiation_id IN (SELECT id FROM negotiations WHERE initiator_org_id IN ({ids}) OR counterparty_org_id IN ({ids}))"))
    await db.execute(text(f"DELETE FROM negotiations WHERE initiator_org_id IN ({ids}) OR counterparty_org_id IN ({ids})"))
    await db.execute(text(f"DELETE FROM orderbook_orders WHERE id IN ({demo_orders})"))
    await db.execute(text(
        "DELETE FROM inventory_items AS inventory WHERE inventory.supplier_id IN (" + ids + ") "
        "AND EXISTS (SELECT 1 FROM organizations AS organization "
        "WHERE organization.id = inventory.supplier_id AND organization.provenance = 'DEMO')"
    ))

# ---------------------------------------------------------------------------
# Fake organization IDs (buyers and suppliers)
# ---------------------------------------------------------------------------
_SLICE_SCHEME_NS = uuid.UUID("c4d5e6f7-a8b9-4012-9abc-def123456789")

BUYER_ORGS = [
    {"id": organization_id, "name": name}
    for organization_id, name in DEMO_SEED_BUYERS
]

SUPPLIER_ORGS = [
    {"id": organization_id, "name": name, "tier": tier}
    for organization_id, name, tier in DEMO_SEED_SUPPLIERS
]

# ---------------------------------------------------------------------------
# Indicative demo bands as at 2026-07-28, in USD/MT. These are not Verdaxis
# assessments. Public anchors: S&P low-carbon methanol reporting, USGBC
# ethanol FOB/C&F data, and IRENA renewable-methanol production-cost ranges.
# Synthetic ethanol is modelled at a premium because no liquid public marine
# benchmark exists.
# Product name -> delivery point -> (bid_lo, bid_hi, ask_lo, ask_hi)
# ---------------------------------------------------------------------------
PRICING: dict[str, dict[str, tuple[float, float, float, float]]] = {
    "Bio Methanol": {
        "Dalian": (890, 930, 950, 990),
        "Busan": (910, 950, 970, 1010),
        "Shanghai": (885, 925, 945, 985),
        "Singapore": (930, 975, 995, 1040),
        "Rotterdam": (900, 950, 975, 1025),
        "Houston": (860, 910, 935, 985),
        "Los Angeles": (900, 950, 975, 1025),
        "Santos": (820, 870, 895, 945),
    },
    "e-Methanol": {
        "Dalian": (1040, 1110, 1140, 1210),
        "Busan": (1070, 1140, 1170, 1240),
        "Shanghai": (1010, 1080, 1110, 1180),
        "Singapore": (1090, 1170, 1205, 1285),
        "Rotterdam": (1190, 1280, 1320, 1410),
        "Houston": (1040, 1120, 1155, 1235),
        "Los Angeles": (1110, 1190, 1225, 1305),
        "Santos": (990, 1070, 1105, 1185),
    },
    "Bio Ethanol": {
        "Dalian": (800, 840, 855, 895),
        "Busan": (810, 850, 865, 905),
        "Shanghai": (795, 835, 850, 890),
        "Singapore": (815, 855, 870, 910),
        "Rotterdam": (780, 820, 835, 875),
        "Houston": (680, 710, 725, 755),
        "Los Angeles": (740, 780, 795, 835),
        "Santos": (730, 760, 775, 805),
    },
    "Synthetic Ethanol": {
        "Dalian": (1140, 1210, 1245, 1315),
        "Busan": (1170, 1240, 1275, 1345),
        "Shanghai": (1120, 1190, 1225, 1295),
        "Singapore": (1190, 1260, 1295, 1365),
        "Rotterdam": (1240, 1320, 1360, 1440),
        "Houston": (1070, 1140, 1175, 1245),
        "Los Angeles": (1130, 1200, 1235, 1305),
        "Santos": (1090, 1160, 1195, 1265),
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

DEMO_BUYER_ORG_ID = DEMO_ACCOUNT_BUYER_ORG_ID
DEMO_SELLER_ORG_ID = DEMO_ACCOUNT_SELLER_ORG_ID

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
    ("Synthetic Ethanol", "Houston", "2026-05", Decimal("900"), Decimal("709.50"), TradeStatus.CONFIRMED, Initiator.SELLER, 8),
    ("Bio Methanol", "Los Angeles", "2026-Q3", Decimal("1800"), Decimal("934.00"), TradeStatus.CONFIRMED, Initiator.BUYER, 15),
]

def build_seed_windows(reference_date: date | None = None, *, quarter_count: int = 6) -> list[str]:
    return tradable_availability_windows(
        today=reference_date or date.today(),
        quarter_count=quarter_count,
    )


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
    "Prefer suppliers with Rotterdam/Houston/Los Angeles delivery capability",
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

async def seed_market_data(
    db: AsyncSession,
    *,
    force_reset: bool = False,
    allow_demo_reset: bool = False,
) -> None:
    """Seed realistic market data. Idempotent unless force_reset is requested."""

    environment = os.environ.get("ENVIRONMENT", "production").strip().lower()
    marker = (await db.execute(
        select(SeedRun).where(SeedRun.seed_name == MARKET_SEED_NAME, SeedRun.environment == environment)
    )).scalar_one_or_none()
    if marker is not None and not force_reset:
        print("[market_seed] Seed run marker found — already seeded, skipping.")
        return

    if force_reset:
        if environment == "production":
            raise RuntimeError("demo reset is categorically forbidden in production")
        if not allow_demo_reset:
            raise RuntimeError("demo reset requires explicit opt-in")
        bind = db.get_bind()
        current_database = str(
            (await db.execute(text("SELECT current_database()"))).scalar_one()
        )
        validate_demo_reset(
            environment=environment,
            explicit_opt_in=allow_demo_reset,
            database_url=str(bind.url),
            current_database=current_database,
        )
        print("[market_seed] Explicit synthetic reset requested.")
        await reset_demo_market_data(db)

    # ------------------------------------------------------------------
    # Step 0: Clear old test data (reverse FK order)
    # ------------------------------------------------------------------
    print("[market_seed] Preserving existing real market data...")

    # ------------------------------------------------------------------
    # Step 1: Ensure seed organizations exist
    # ------------------------------------------------------------------
    print("[market_seed] Ensuring seed organizations...")
    for org in BUYER_ORGS:
        await db.execute(text(
            "INSERT INTO organizations (id, name, type, verification_status, provenance) "
            "VALUES (:id, :name, 'SHIPPING_LINE', 'APPROVED', 'DEMO') "
            "ON CONFLICT (id) DO NOTHING"
        ), {"id": org["id"], "name": org["name"]})

    for org in SUPPLIER_ORGS:
        await db.execute(text(
            "INSERT INTO organizations (id, name, type, supplier_tier, verification_status, provenance) "
            "VALUES (:id, :name, 'FUEL_SUPPLIER', :tier, 'APPROVED', 'DEMO') "
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

                order = _seed_order(
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

                order = _seed_order(
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

    total_orders = len(all_orders)
    print(f"[market_seed] Created {total_orders} synthetic orders.")

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
                    order = _seed_order(
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
                    order = _seed_order(
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

        trade = _seed_trade(
            id=uuid.uuid4(),
            bid_order_id=bid.id,
            ask_order_id=ask.id,
            buyer_id=bid.organization_id,
            seller_id=ask.organization_id,
            product_id=bid.product_id,
            product_name=bid.product_name,
            fuel_type=bid.fuel_type,
            fuel_grade=bid.fuel_grade,
            market_product=bid.market_product,
            delivery_point_id=bid.delivery_point_id,
            delivery_point_name=bid.delivery_point_name,
            delivery_point_region=bid.region,
            availability_window=bid.availability_window,
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
                    order = _seed_order(
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
                    order = _seed_order(
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
        ("Bio Methanol",      "Houston",   2000, 875.00),
        ("Bio Methanol",      "Singapore", 3000, 1065.00),
        ("Bio Ethanol",       "Santos",    2500, 555.00),
        ("Bio Ethanol",       "Shanghai",  1800, None),
        ("Synthetic Ethanol", "Rotterdam", 1500, 720.00),
        ("Synthetic Ethanol", "Singapore", 2000, None),
        ("e-Methanol",        "Busan",     2500, 1150.00),
        ("e-Methanol",        "Rotterdam", 1000, None),
        ("e-Methanol",        "Dalian",    1500, 890.00),
        ("Synthetic Ethanol", "Los Angeles", 2000, 735.00),
    ]

    for i, (product_name, port_name, qty, target_price) in enumerate(rfq_configs):
        buyer = _RNG.choice(BUYER_ORGS)
        status = _RNG.choices(
            [RFQStatus.OPEN, RFQStatus.QUOTED, RFQStatus.EXPIRED],
            weights=[40, 45, 15],
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
        if status == RFQStatus.QUOTED:
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

                quote = RFQQuote(
                    id=uuid.uuid4(),
                    rfq_id=rfq.id,
                    seller_org_id=seller["id"],
                    price_per_mt_usd=quote_price,
                    notes=_RNG.choice(QUOTE_NOTES),
                    status=QuoteStatus.PENDING,
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
        order = _seed_order(
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
        order = _seed_order(
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
    print("[market_seed] Creating deterministic demo-account trades...")

    await db.execute(text(
        "INSERT INTO organizations (id, name, type, verification_status, provenance) "
        "VALUES (:id, :name, 'SHIPPING_LINE', 'APPROVED', 'DEMO') "
        "ON CONFLICT (id) DO NOTHING"
    ), {"id": DEMO_BUYER_ORG_ID, "name": "Verdaxis Demo Buyer 06"})
    await db.execute(text(
        "INSERT INTO organizations (id, name, type, supplier_tier, verification_status, provenance) "
        "VALUES (:id, :name, 'FUEL_SUPPLIER', 'REGIONAL_SUPPLIER', 'APPROVED', 'DEMO') "
        "ON CONFLICT (id) DO NOTHING"
    ), {"id": DEMO_SELLER_ORG_ID, "name": "Verdaxis Demo Supplier 06"})
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
        bid_order = _seed_order(
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
        ask_order = _seed_order(
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

        trade = _seed_trade(
            id=uuid.uuid4(),
            bid_order_id=bid_order.id,
            ask_order_id=ask_order.id,
            buyer_id=DEMO_BUYER_ORG_ID,
            seller_id=DEMO_SELLER_ORG_ID,
            product_id=bid_order.product_id,
            product_name=bid_order.product_name,
            fuel_type=bid_order.fuel_type,
            fuel_grade=bid_order.fuel_grade,
            market_product=bid_order.market_product,
            delivery_point_id=bid_order.delivery_point_id,
            delivery_point_name=bid_order.delivery_point_name,
            delivery_point_region=bid_order.region,
            availability_window=bid_order.availability_window,
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
    if marker is None:
        db.add(SeedRun(seed_name=MARKET_SEED_NAME, environment=environment, run_metadata={"provenance": "DEMO"}))
    else:
        marker.completed_at = datetime.now(timezone.utc)
        marker.run_metadata = {"provenance": "DEMO"}
    await db.commit()
    print("[market_seed] Done. Market data seeded successfully.")

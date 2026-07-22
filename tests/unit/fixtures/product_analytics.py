"""Frozen Product Analytics metric scenario and expected values.

This module is the contract fixture for the Product Analytics workspace
(`fe/docs/plans/2026-07-15-product-analytics-workspace.md`). It freezes:

1. A deterministic database scenario (organizations, users in every lifecycle
   state, bids/asks, trades in every status, commissions) anchored to fixed
   UTC instants so expected values never drift with wall-clock time.
2. Hand-derived expected outputs for every metric definition in plan §1.5,
   with the arithmetic recorded next to each value. Task 3 service tests
   assert against ``EXPECTED``; a deliberate definition change must update
   the derivation comment here in the same commit.
3. Planned fact-table rows (login days, status transitions) as plain data.
   Task 5 turns these into ORM rows; keeping them as data here lets this
   module import before those models exist.
4. Canned Umami payloads for the existing aggregate contract and the
   proposed event-data property routes, shared by MockTransport tests and
   the read-only staging smoke.

Reporting periods are half-open UTC intervals ``[start, end)``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import UTC, date, datetime
from decimal import Decimal
from uuid import UUID, uuid5

from sqlalchemy import Column, Table, text
from sqlalchemy.dialects.postgresql import UUID as PgUUID
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.market_catalog import MarketProduct
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import (
    Initiator,
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.models.orders import Commission, CommissionStatus
from app.models.product_analytics import UserLoginDay, UserStatusTransition
from app.models.user import Organization, OrgType, User, UserRole, UserStatus, OrganizationProvenance
from app.services.demo_market import (
    DEMO_ACTIVITY_BUYER_ORG_ID,
    DEMO_ACTIVITY_SELLER_ORG_ID,
)

_NAMESPACE = UUID("6e3b4f52-9c1d-4f7a-9a2e-8f5d0c1b2a30")


def fixture_uuid(name: str) -> UUID:
    """Deterministic, human-traceable UUID for a named fixture entity."""
    return uuid5(_NAMESPACE, name)


def _utc(year: int, month: int, day: int, hour: int = 0, minute: int = 0) -> datetime:
    return datetime(year, month, day, hour, minute, tzinfo=UTC)


# ---------------------------------------------------------------------------
# Reporting anchors (plan §1.4 rule 16: half-open UTC intervals)
# ---------------------------------------------------------------------------

PERIOD_START = _utc(2026, 6, 1)
PERIOD_END = _utc(2026, 7, 1)  # also the liquidity/qualification as-of instant
PREVIOUS_START = _utc(2026, 5, 2)
PREVIOUS_END = _utc(2026, 6, 1)

# ---------------------------------------------------------------------------
# Entity ids
# ---------------------------------------------------------------------------

LIVE_BUYER_ORG_ID = fixture_uuid("org:live-buyer")
LIVE_SUPPLIER_ORG_ID = fixture_uuid("org:live-supplier")
RETURNING_ORG_ID = fixture_uuid("org:returning-trader")
PENDING_ORG_ID = fixture_uuid("org:pending-prospect")
DEMO_ORG_ID = DEMO_ACTIVITY_BUYER_ORG_ID  # member of DEMO_MARKET_ORG_IDS
DEMO_SUPPLIER_ORG_ID = DEMO_ACTIVITY_SELLER_ORG_ID

USER_IDS = {
    "buyer_active": fixture_uuid("user:buyer-active"),
    "buyer_new": fixture_uuid("user:buyer-new"),
    "buyer_never_logged": fixture_uuid("user:buyer-never-logged"),
    "supplier_active": fixture_uuid("user:supplier-active"),
    "supplier_rejected": fixture_uuid("user:supplier-rejected"),
    "trader_active": fixture_uuid("user:trader-active"),
    "pending_buyer": fixture_uuid("user:pending-buyer"),
    "admin": fixture_uuid("user:admin"),
    "demo_buyer": fixture_uuid("user:demo-buyer"),
}

PRODUCT_BIO_METHANOL_ID = PRODUCTS_BY_NAME["Bio Methanol"].id
PRODUCT_E_METHANOL_ID = PRODUCTS_BY_NAME["e-Methanol"].id
DELIVERY_POINT_SINGAPORE_ID = DELIVERY_POINTS_BY_NAME["Singapore"].id
DELIVERY_POINT_ROTTERDAM_ID = DELIVERY_POINTS_BY_NAME["Rotterdam"].id

ORDER_IDS = {
    "bid_live_open": fixture_uuid("order:bid-live-open"),
    "bid_live_filled": fixture_uuid("order:bid-live-filled"),
    "bid_live_cancelled": fixture_uuid("order:bid-live-cancelled"),
    "ask_live_open": fixture_uuid("order:ask-live-open"),
    "ask_live_filled": fixture_uuid("order:ask-live-filled"),
    "ask_live_partial": fixture_uuid("order:ask-live-partial"),
    "ask_live_expired": fixture_uuid("order:ask-live-expired"),
    "bid_returning_open": fixture_uuid("order:bid-returning-open"),
    "bid_returning_old": fixture_uuid("order:bid-returning-old"),
    "bid_unknown_open": fixture_uuid("order:bid-unknown-open"),
    "bid_demo_open": fixture_uuid("order:bid-demo-open"),
}

TRADE_IDS = {
    "paid": fixture_uuid("trade:paid"),
    "confirmed": fixture_uuid("trade:confirmed"),
    "delivered_previous": fixture_uuid("trade:delivered-previous"),
    "pending": fixture_uuid("trade:pending"),
    "cancelled": fixture_uuid("trade:cancelled"),
    "declined": fixture_uuid("trade:declined"),
    "legacy_confirmed": fixture_uuid("trade:legacy-confirmed"),
    "paid_missing_paid_at": fixture_uuid("trade:paid-missing-paid-at"),
    "demo_mixed": fixture_uuid("trade:demo-mixed"),
}

LEGACY_MATCH_IDS = {
    "paid": fixture_uuid("legacy-order:paid"),
    "confirmed": fixture_uuid("legacy-order:confirmed"),
    "delivered_previous": fixture_uuid("legacy-order:delivered-previous"),
    "paid_missing_paid_at": fixture_uuid("legacy-order:paid-missing-paid-at"),
}


def legacy_orders_stub_table() -> Table:
    """Minimal stand-in for the legacy ``orders`` table.

    ``Commission.match_id`` references ``orders.id`` but the legacy model was
    removed; only Alembic-migrated databases still carry the table. Unit
    fixtures need a resolvable FK target to emit DDL.
    """
    existing = Base.metadata.tables.get("orders")
    if existing is not None:
        return existing
    return Table(
        "orders",
        Base.metadata,
        Column("id", PgUUID(as_uuid=True), primary_key=True),
    )


SCENARIO_TABLES = (
    Organization.__table__,
    User.__table__,
    Product.__table__,
    DeliveryPoint.__table__,
    OrderBookOrder.__table__,
    Trade.__table__,
    Commission.__table__,
)


def scenario_tables() -> tuple[Table, ...]:
    """All tables the scenario writes, legacy stub included, creation-ordered."""
    return (
        *SCENARIO_TABLES[:-1],
        legacy_orders_stub_table(),
        Commission.__table__,
        UserLoginDay.__table__,
        UserStatusTransition.__table__,
    )


# ---------------------------------------------------------------------------
# Scenario builders
# ---------------------------------------------------------------------------


@dataclass
class ProductAnalyticsScenario:
    organizations: list[Organization] = field(default_factory=list)
    users: list[User] = field(default_factory=list)
    products: list[Product] = field(default_factory=list)
    delivery_points: list[DeliveryPoint] = field(default_factory=list)
    orders: list[OrderBookOrder] = field(default_factory=list)
    trades: list[Trade] = field(default_factory=list)
    commissions: list[Commission] = field(default_factory=list)
    legacy_match_rows: list[dict[str, UUID]] = field(default_factory=list)


def build_scenario() -> ProductAnalyticsScenario:
    """Fresh ORM objects for the frozen scenario (one instance set per session)."""
    scenario = ProductAnalyticsScenario()

    scenario.organizations = [
        # Explicit created_at values keep organization-to-first-order duration
        # metrics deterministic.
        Organization(id=LIVE_BUYER_ORG_ID, name="Live Buyer Shipping",
                     type=OrgType.FUEL_BUYER, provenance=OrganizationProvenance.REAL, created_at=_utc(2026, 5, 9)),
        Organization(id=LIVE_SUPPLIER_ORG_ID, name="Live Supplier Fuels",
                     type=OrgType.FUEL_SUPPLIER, provenance=OrganizationProvenance.REAL, created_at=_utc(2026, 3, 10)),
        Organization(id=RETURNING_ORG_ID, name="Returning Trader",
                     type=OrgType.FUEL_TRADER, provenance=OrganizationProvenance.REAL, created_at=_utc(2026, 3, 15)),
        Organization(id=PENDING_ORG_ID, name="Pending Prospect",
                     type=OrgType.FUEL_BUYER, created_at=_utc(2026, 6, 4)),
        Organization(id=DEMO_ORG_ID, name="Demo Activity Buyer",
                     type=OrgType.FUEL_BUYER, provenance=OrganizationProvenance.DEMO, created_at=_utc(2026, 6, 1)),
        Organization(id=DEMO_SUPPLIER_ORG_ID, name="Demo Activity Supplier",
                     type=OrgType.FUEL_SUPPLIER, provenance=OrganizationProvenance.DEMO, created_at=_utc(2026, 6, 1)),
    ]

    def user(
        key: str,
        role: UserRole,
        status: UserStatus,
        org_id: UUID | None,
        created_at: datetime,
        last_login: datetime | None = None,
    ) -> User:
        return User(
            id=USER_IDS[key],
            email=f"{key.replace('_', '-')}@fixtures.verdaxis.test",
            password_hash="fixture-hash",
            role=role,
            status=status,
            organization_id=org_id,
            created_at=created_at,
            last_login=last_login,
            # Verified by default so the activation drop-off buckets stay
            # mutually exclusive and deterministic.
            email_verified=True,
        )

    scenario.users = [
        # Lifecycle coverage: approved+active, approved new in period, approved
        # never logged in, rejected, pending, admin, demo-org member.
        user("buyer_active", UserRole.BUYER, UserStatus.APPROVED, LIVE_BUYER_ORG_ID,
             _utc(2026, 5, 10, 9), _utc(2026, 6, 20, 8)),
        user("buyer_new", UserRole.BUYER, UserStatus.APPROVED, LIVE_BUYER_ORG_ID,
             _utc(2026, 6, 10, 10)),
        user("buyer_never_logged", UserRole.BUYER, UserStatus.APPROVED, LIVE_BUYER_ORG_ID,
             _utc(2026, 3, 20, 9)),
        user("supplier_active", UserRole.SUPPLIER, UserStatus.APPROVED, LIVE_SUPPLIER_ORG_ID,
             _utc(2026, 3, 15, 9), _utc(2026, 6, 15, 7)),
        user("supplier_rejected", UserRole.SUPPLIER, UserStatus.REJECTED, LIVE_SUPPLIER_ORG_ID,
             _utc(2026, 5, 5, 8)),
        user("trader_active", UserRole.BUYER, UserStatus.APPROVED, RETURNING_ORG_ID,
             _utc(2026, 3, 18, 9), _utc(2026, 6, 21, 6)),
        user("pending_buyer", UserRole.BUYER, UserStatus.PENDING, PENDING_ORG_ID,
             _utc(2026, 6, 5, 11)),
        user("admin", UserRole.ADMIN, UserStatus.APPROVED, None,
             _utc(2026, 3, 1, 9), _utc(2026, 6, 12, 9)),
        user("demo_buyer", UserRole.BUYER, UserStatus.APPROVED, DEMO_ORG_ID,
             _utc(2026, 6, 8, 9), _utc(2026, 6, 18, 9)),
    ]

    scenario.products = [
        Product(id=PRODUCT_BIO_METHANOL_ID, name="Bio Methanol",
                fuel_type="Methanol", fuel_grade="Bio", is_active=True),
        Product(id=PRODUCT_E_METHANOL_ID, name="e-Methanol",
                fuel_type="Methanol", fuel_grade="E", is_active=True),
    ]
    scenario.delivery_points = [
        DeliveryPoint(id=DELIVERY_POINT_SINGAPORE_ID, name="Singapore", region="Asia", is_active=True),
        DeliveryPoint(id=DELIVERY_POINT_ROTTERDAM_ID, name="Rotterdam", region="Europe", is_active=True),
    ]

    def order(
        key: str,
        org_id: UUID,
        side: OrderSide,
        status: OrderBookStatus,
        created_at: datetime,
        price: str,
        quantity: str,
        remaining: str,
        product_id: UUID = PRODUCT_BIO_METHANOL_ID,
        delivery_point_id: UUID | None = DELIVERY_POINT_SINGAPORE_ID,
        expires_at: datetime | None = None,
    ) -> OrderBookOrder:
        return OrderBookOrder(
            id=ORDER_IDS[key],
            organization_id=org_id,
            provenance=(OrganizationProvenance.DEMO if org_id == DEMO_ORG_ID else (OrganizationProvenance.UNKNOWN if org_id == PENDING_ORG_ID else OrganizationProvenance.REAL)),
            side=side,
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            quantity_mt=Decimal(quantity),
            remaining_quantity_mt=Decimal(remaining),
            price_per_mt_usd=Decimal(price),
            availability_window="SPOT",
            status=status,
            created_at=created_at,
            expires_at=expires_at,
        )

    scenario.orders = [
        # Live buyer org — 3 orders in the current period.
        order("bid_live_open", LIVE_BUYER_ORG_ID, OrderSide.BID, OrderBookStatus.OPEN,
              _utc(2026, 6, 7, 12), price="780", quantity="500", remaining="500"),
        order("bid_live_filled", LIVE_BUYER_ORG_ID, OrderSide.BID, OrderBookStatus.FILLED,
              _utc(2026, 6, 4, 12), price="790", quantity="400", remaining="0"),
        order("bid_live_cancelled", LIVE_BUYER_ORG_ID, OrderSide.BID, OrderBookStatus.CANCELLED,
              _utc(2026, 6, 9, 12), price="760", quantity="200", remaining="200"),
        # Live supplier org — 3 orders in the current period, 1 previous.
        order("ask_live_open", LIVE_SUPPLIER_ORG_ID, OrderSide.ASK, OrderBookStatus.OPEN,
              _utc(2026, 6, 5, 12), price="800", quantity="1000", remaining="1000"),
        order("ask_live_filled", LIVE_SUPPLIER_ORG_ID, OrderSide.ASK, OrderBookStatus.FILLED,
              _utc(2026, 6, 3, 12), price="790", quantity="400", remaining="0"),
        order("ask_live_partial", LIVE_SUPPLIER_ORG_ID, OrderSide.ASK, OrderBookStatus.PARTIALLY_FILLED,
              _utc(2026, 6, 8, 12), price="810", quantity="500", remaining="250",
              delivery_point_id=DELIVERY_POINT_ROTTERDAM_ID),
        order("ask_live_expired", LIVE_SUPPLIER_ORG_ID, OrderSide.ASK, OrderBookStatus.EXPIRED,
              _utc(2026, 5, 15, 12), price="900", quantity="300", remaining="300",
              product_id=PRODUCT_E_METHANOL_ID, expires_at=_utc(2026, 5, 25, 12)),
        # Returning trader — active before the previous period and again now.
        order("bid_returning_open", RETURNING_ORG_ID, OrderSide.BID, OrderBookStatus.OPEN,
              _utc(2026, 6, 21, 12), price="775", quantity="200", remaining="200"),
        order("bid_returning_old", RETURNING_ORG_ID, OrderSide.BID, OrderBookStatus.FILLED,
              _utc(2026, 3, 20, 12), price="850", quantity="100", remaining="0",
              product_id=PRODUCT_E_METHANOL_ID),
        # Pending-only org: provenance UNKNOWN (no approved market member).
        order("bid_unknown_open", PENDING_ORG_ID, OrderSide.BID, OrderBookStatus.OPEN,
              _utc(2026, 6, 11, 12), price="700", quantity="50", remaining="50"),
        # Demo org: provenance DEMO, never mixed into live sections.
        order("bid_demo_open", DEMO_ORG_ID, OrderSide.BID, OrderBookStatus.OPEN,
              _utc(2026, 6, 6, 12), price="770", quantity="100", remaining="100",
              expires_at=_utc(2026, 7, 2)),
    ]

    def trade(
        key: str,
        status: TradeStatus,
        created_at: datetime,
        quantity: str,
        price: str,
        *,
        buyer_id: UUID = LIVE_BUYER_ORG_ID,
        seller_id: UUID = LIVE_SUPPLIER_ORG_ID,
        bid_order_key: str | None = None,
        ask_order_key: str | None = None,
        confirmed_at: datetime | None = None,
        delivered_at: datetime | None = None,
        paid_at: datetime | None = None,
        final_quantity: str | None = None,
        final_price: str | None = None,
        final_total: str | None = None,
        commission_amount: str | None = None,
    ) -> Trade:
        demo_ids = {DEMO_ORG_ID, DEMO_SUPPLIER_ORG_ID}
        return Trade(
            id=TRADE_IDS[key],
            bid_order_id=ORDER_IDS[bid_order_key] if bid_order_key else None,
            ask_order_id=ORDER_IDS[ask_order_key] if ask_order_key else None,
            buyer_id=buyer_id,
            seller_id=seller_id,
            initiator_org_id=buyer_id,
            buyer_provenance=(OrganizationProvenance.DEMO if buyer_id in demo_ids else OrganizationProvenance.REAL),
            seller_provenance=(OrganizationProvenance.DEMO if seller_id in demo_ids else OrganizationProvenance.REAL),
            initiated_by=Initiator.BUYER,
            product_id=PRODUCT_BIO_METHANOL_ID,
            product_name="Bio Methanol",
            fuel_type="Methanol",
            fuel_grade="Bio",
            market_product=MarketProduct.BIO_METHANOL.value,
            delivery_point_id=DELIVERY_POINT_SINGAPORE_ID,
            delivery_point_name="Singapore",
            delivery_point_region="Asia",
            availability_window="SPOT",
            market_snapshot_version=1,
            quantity_mt=Decimal(quantity),
            price_per_mt_usd=Decimal(price),
            status=status,
            final_quantity_mt=Decimal(final_quantity) if final_quantity else None,
            final_price_per_mt=Decimal(final_price) if final_price else None,
            final_total_usd=Decimal(final_total) if final_total else None,
            commission_amount_usd=Decimal(commission_amount) if commission_amount else None,
            confirmed_at=confirmed_at,
            delivered_at=delivered_at,
            paid_at=paid_at,
            created_at=created_at,
        )

    scenario.trades = [
        trade("paid", TradeStatus.PAID, _utc(2026, 6, 5, 12), "400", "790",
              bid_order_key="bid_live_filled", ask_order_key="ask_live_filled",
              confirmed_at=_utc(2026, 6, 6, 9), delivered_at=_utc(2026, 6, 10, 9),
              paid_at=_utc(2026, 6, 20, 9), final_quantity="400", final_price="790",
              final_total="316000", commission_amount="1580"),
        trade("confirmed", TradeStatus.CONFIRMED, _utc(2026, 6, 11, 12), "250", "810",
              ask_order_key="ask_live_partial", confirmed_at=_utc(2026, 6, 12, 10)),
        trade("delivered_previous", TradeStatus.DELIVERED, _utc(2026, 5, 18, 12), "300", "795",
              confirmed_at=_utc(2026, 5, 20, 10), delivered_at=_utc(2026, 5, 28, 10),
              final_quantity="290", final_price="795", final_total="230550"),
        trade("pending", TradeStatus.PENDING_CONFIRMATION, _utc(2026, 6, 14, 12), "150", "805"),
        trade("cancelled", TradeStatus.CANCELLED, _utc(2026, 6, 8, 12), "100", "800"),
        trade("declined", TradeStatus.DECLINED, _utc(2026, 6, 9, 12), "120", "802"),
        # Orderless execution: the immutable market snapshot, not an order
        # relationship, carries its historical identity.
        trade("legacy_confirmed", TradeStatus.CONFIRMED, _utc(2026, 6, 18, 12), "80", "799",
              confirmed_at=_utc(2026, 6, 18, 12)),
        # Paid in the following period, so it is confirmed activity in this
        # period without contributing current-period realized GMV.
        trade("paid_missing_paid_at", TradeStatus.PAID, _utc(2026, 6, 15, 12), "120", "790",
              confirmed_at=_utc(2026, 6, 16, 9), delivered_at=_utc(2026, 7, 1, 9),
              paid_at=_utc(2026, 7, 2, 9), final_quantity="120", final_price="790",
              final_total="94800"),
        # Exact allowlisted DEMO/DEMO activity, never mixed into live metrics.
        trade("demo_mixed", TradeStatus.CONFIRMED, _utc(2026, 6, 14, 12), "100", "785",
              buyer_id=DEMO_ORG_ID, seller_id=DEMO_SUPPLIER_ORG_ID,
              confirmed_at=_utc(2026, 6, 15, 10)),
    ]

    scenario.legacy_match_rows = [{"id": match_id} for match_id in LEGACY_MATCH_IDS.values()]

    def commission(
        trade_key: str,
        status: CommissionStatus,
        amount: str,
        payment_date: date | None = None,
    ) -> Commission:
        return Commission(
            id=fixture_uuid(f"commission:{trade_key}"),
            match_id=LEGACY_MATCH_IDS[trade_key],
            trade_id=TRADE_IDS[trade_key],
            amount_usd=Decimal(amount),
            status=status,
            payment_date=payment_date,
        )

    scenario.commissions = [
        # 400 MT × 790 USD/MT = 316,000 × 0.5% = 1,580.
        commission("paid", CommissionStatus.PAID, "1580", payment_date=date(2026, 6, 25)),
        # 250 × 810 = 202,500 × 0.5% = 1,012.50 — accrued, not realized.
        commission("confirmed", CommissionStatus.PENDING, "1012.50"),
        # 290 × 795 = 230,550 × 0.5% = 1,152.75 — invoiced, not realized.
        commission("delivered_previous", CommissionStatus.INVOICED, "1152.75"),
        # PAID with no payment_date: excluded from realized revenue and
        # reported via missing_commission_payment_date_count.
        commission("paid_missing_paid_at", CommissionStatus.PAID, "474"),
    ]

    return scenario


async def _insert_legacy_match_rows(session: AsyncSession, scenario: ProductAnalyticsScenario) -> None:
    """Satisfy ``Commission.match_id`` FK targets on both harnesses.

    SQLite uses the single-column stub table; PostgreSQL databases migrated by
    Alembic carry the real legacy ``orders``/``public_listings`` tables (the
    renamed ``rfq_matches`` chain), so minimal legacy rows are inserted there.
    Nothing in Product Analytics reads these legacy tables.
    """
    # Register the metadata stub on both paths: the ORM unit-of-work needs a
    # resolvable FK target to order the commissions insert. On PostgreSQL the
    # real (Alembic-migrated) table is written through raw SQL below.
    orders_table = legacy_orders_stub_table()
    dialect = session.get_bind().dialect.name
    if dialect != "postgresql":
        for row in scenario.legacy_match_rows:
            await session.execute(orders_table.insert().values(**row))
        return

    listing_id = fixture_uuid("legacy-listing:stub")
    await session.execute(
        text(
            "INSERT INTO public_listings (id, supplier_id, region, fuel_type, fuel_grade, "
            "quantity_mt, price_per_mt_usd, availability_window, is_verdaxis_verified, "
            "status, created_at, updated_at) VALUES (:id, :supplier_id, 'Asia', 'Methanol', "
            "'Bio', 1, 1, 'SPOT', false, 'ACTIVE', :ts, :ts)"
        ),
        {"id": listing_id, "supplier_id": LIVE_SUPPLIER_ORG_ID, "ts": PERIOD_START},
    )
    for row in scenario.legacy_match_rows:
        await session.execute(
            text(
                "INSERT INTO orders (id, listing_id, buyer_id, status, "
                "buyer_accepted_terms_at, commission_rate_pct, created_at) "
                "VALUES (:id, :listing_id, :buyer_id, 'COMPLETED', :ts, 0.5, :ts)"
            ),
            {
                "id": row["id"],
                "listing_id": listing_id,
                "buyer_id": LIVE_BUYER_ORG_ID,
                "ts": PERIOD_START,
            },
        )


async def seed_product_analytics_scenario(session: AsyncSession) -> ProductAnalyticsScenario:
    """Insert the frozen scenario and commit. Tables must already exist."""
    scenario = build_scenario()
    if session.get_bind().dialect.name == "postgresql":
        database_name = str(
            (await session.execute(text("SELECT current_database()"))).scalar_one()
        )
        if not database_name.endswith("_analytics_test"):
            raise RuntimeError(
                "REAL analytics fixtures require a disposable *_analytics_test database"
            )
        # REAL provenance is assigned only by the externally approved operator
        # boundary. The disposable database owner models that pre-approved
        # state without adding an application promotion surface.
        await session.execute(text("SET LOCAL session_replication_role = 'replica'"))
        session.add_all(scenario.organizations)
        await session.flush()
        await session.execute(text("SET LOCAL session_replication_role = 'origin'"))
    else:
        session.add_all(scenario.organizations)
    session.add_all(scenario.users)
    session.add_all(scenario.products)
    session.add_all(scenario.delivery_points)
    await session.flush()
    session.add_all(scenario.orders)
    await session.flush()
    session.add_all(scenario.trades)
    await session.flush()
    await _insert_legacy_match_rows(session, scenario)
    session.add_all(scenario.commissions)
    await session.commit()
    return scenario


async def seed_fact_tables(session: AsyncSession) -> None:
    """Insert the planned login-day and status-transition facts (Task 5).

    Kept separate from the base scenario so pre-fact tests can still exercise
    the coverage-gated null behavior by seeding only the scenario.
    """
    for planned in PLANNED_LOGIN_DAYS:
        user_key = planned["user"]
        user_id = USER_IDS[user_key]
        org_id, role = _USER_SNAPSHOTS[user_key]
        session.add(
            UserLoginDay(
                id=fixture_uuid(f"login-day:{user_key}:{planned['activity_date']}"),
                activity_date=planned["activity_date"],
                user_id=user_id,
                organization_id=org_id,
                role=role,
                login_count=planned["login_count"],
                first_login_at=planned["first_login_at"],
                last_login_at=planned["last_login_at"],
            )
        )
    for planned in PLANNED_STATUS_TRANSITIONS:
        user_key = planned["user"]
        org_id, role = _USER_SNAPSHOTS[user_key]
        session.add(
            UserStatusTransition(
                id=fixture_uuid(
                    f"transition:{user_key}:{planned['to']}:{planned['effective_at']}"
                ),
                user_id=USER_IDS[user_key],
                organization_id=org_id,
                role=role,
                from_status=UserStatus(planned["from"]) if planned["from"] else None,
                to_status=UserStatus(planned["to"]),
                effective_at=planned["effective_at"],
                provenance=planned["provenance"],
            )
        )
    await session.commit()


# Organization/role snapshots for fact rows, keyed by user fixture name.
_USER_SNAPSHOTS: dict[str, tuple[UUID | None, UserRole]] = {
    "buyer_active": (LIVE_BUYER_ORG_ID, UserRole.BUYER),
    "buyer_new": (LIVE_BUYER_ORG_ID, UserRole.BUYER),
    "buyer_never_logged": (LIVE_BUYER_ORG_ID, UserRole.BUYER),
    "supplier_active": (LIVE_SUPPLIER_ORG_ID, UserRole.SUPPLIER),
    "supplier_rejected": (LIVE_SUPPLIER_ORG_ID, UserRole.SUPPLIER),
    "trader_active": (RETURNING_ORG_ID, UserRole.BUYER),
    "pending_buyer": (PENDING_ORG_ID, UserRole.BUYER),
    "admin": (None, UserRole.ADMIN),
    "demo_buyer": (DEMO_ORG_ID, UserRole.BUYER),
}


# ---------------------------------------------------------------------------
# Planned fact rows (Task 5 models) as plain data
# ---------------------------------------------------------------------------

# (user_key, activity_date, login_count, first_login_at, last_login_at)
PLANNED_LOGIN_DAYS: tuple[dict[str, object], ...] = (
    {"user": "buyer_active", "activity_date": date(2026, 5, 20), "login_count": 1,
     "first_login_at": _utc(2026, 5, 20, 8), "last_login_at": _utc(2026, 5, 20, 8)},
    {"user": "buyer_active", "activity_date": date(2026, 6, 10), "login_count": 2,
     "first_login_at": _utc(2026, 6, 10, 7), "last_login_at": _utc(2026, 6, 10, 16)},
    {"user": "buyer_active", "activity_date": date(2026, 6, 20), "login_count": 1,
     "first_login_at": _utc(2026, 6, 20, 8), "last_login_at": _utc(2026, 6, 20, 8)},
    {"user": "supplier_active", "activity_date": date(2026, 6, 15), "login_count": 1,
     "first_login_at": _utc(2026, 6, 15, 7), "last_login_at": _utc(2026, 6, 15, 7)},
    {"user": "trader_active", "activity_date": date(2026, 6, 21), "login_count": 1,
     "first_login_at": _utc(2026, 6, 21, 6), "last_login_at": _utc(2026, 6, 21, 6)},
    # Stored but excluded from member metrics (plan §1.4 rules 4 and 5).
    {"user": "admin", "activity_date": date(2026, 6, 12), "login_count": 1,
     "first_login_at": _utc(2026, 6, 12, 9), "last_login_at": _utc(2026, 6, 12, 9)},
    {"user": "demo_buyer", "activity_date": date(2026, 6, 18), "login_count": 1,
     "first_login_at": _utc(2026, 6, 18, 9), "last_login_at": _utc(2026, 6, 18, 9)},
)

# (user_key, from_status, to_status, effective_at, provenance)
PLANNED_STATUS_TRANSITIONS: tuple[dict[str, object], ...] = (
    {"user": "buyer_active", "from": None, "to": "PENDING",
     "effective_at": _utc(2026, 5, 10, 9), "provenance": "workflow"},
    {"user": "buyer_active", "from": "PENDING", "to": "APPROVED",
     "effective_at": _utc(2026, 5, 12, 9), "provenance": "workflow"},
    {"user": "buyer_new", "from": None, "to": "PENDING",
     "effective_at": _utc(2026, 6, 10, 10), "provenance": "workflow"},
    {"user": "buyer_new", "from": "PENDING", "to": "APPROVED",
     "effective_at": _utc(2026, 6, 12, 10), "provenance": "workflow"},
    {"user": "buyer_never_logged", "from": None, "to": "APPROVED",
     "effective_at": _utc(2026, 5, 1), "provenance": "migration_snapshot"},
    {"user": "supplier_active", "from": None, "to": "APPROVED",
     "effective_at": _utc(2026, 5, 1), "provenance": "migration_snapshot"},
    {"user": "supplier_rejected", "from": None, "to": "PENDING",
     "effective_at": _utc(2026, 5, 5, 8), "provenance": "workflow"},
    {"user": "supplier_rejected", "from": "PENDING", "to": "REJECTED",
     "effective_at": _utc(2026, 5, 8, 8), "provenance": "workflow"},
    {"user": "trader_active", "from": None, "to": "APPROVED",
     "effective_at": _utc(2026, 5, 1), "provenance": "migration_snapshot"},
    {"user": "pending_buyer", "from": None, "to": "PENDING",
     "effective_at": _utc(2026, 6, 5, 11), "provenance": "workflow"},
    {"user": "demo_buyer", "from": None, "to": "PENDING",
     "effective_at": _utc(2026, 6, 8, 9), "provenance": "workflow"},
    {"user": "demo_buyer", "from": "PENDING", "to": "APPROVED",
     "effective_at": _utc(2026, 6, 9, 9), "provenance": "workflow"},
)

# ---------------------------------------------------------------------------
# Expected metric values (plan §1.5 definitions, current period unless noted)
# ---------------------------------------------------------------------------

EXPECTED: dict[str, object] = {
    "members": {
        # Buyer/Supplier users created in [start, end); demo-org users excluded
        # (rule 5): buyer_new (6/10) + pending_buyer (6/5) = 2.
        "registered_current": 2,
        # buyer_active (5/10) + supplier_rejected (5/5) = 2.
        "registered_previous": 2,
        # Distinct members with a login day in the period (PLANNED_LOGIN_DAYS,
        # admin and demo excluded): buyer_active, supplier_active, trader_active.
        "active_current": 3,
        # The previous window [5/2, 6/1) starts before login-history coverage
        # begins (first fact row 5/20), so the metric is null — never
        # fabricated from partial facts or User.last_login. The single
        # recorded previous-window login (buyer_active 5/20) stays internal.
        "active_previous": None,
        # Approved members who never logged in: buyer_never_logged and
        # buyer_new (approved 6/12, no login recorded).
        "dormant_approved": 2,
        # Latest status transition at or before PERIOD_END is APPROVED, org is
        # non-demo, role Buyer/Supplier: live buyer org (buyer_active,
        # buyer_new, buyer_never_logged), live supplier org (supplier_active),
        # returning org (trader_active) → 3 organizations.
        "qualified_organizations_as_of_end": 3,
        # Same reconstruction at PREVIOUS_END: buyer_active approved 5/12,
        # snapshots 5/1 → same 3 organizations.
        "qualified_organizations_as_of_previous_end": 3,
    },
    "activation": {
        # Transition to APPROVED inside [start, end): buyer_new (6/12).
        "approved_in_period": 1,
        # Mutually exclusive drop-off buckets over non-demo Buyer/Supplier
        # users (classified in order): rejected → unverified → pending →
        # organization incomplete → approved-but-never-logged-in.
        "drop_off_rejected": 1,            # supplier_rejected
        "drop_off_unverified": 0,          # all fixture users verified
        "drop_off_pending_approval": 1,    # pending_buyer
        "drop_off_organization_incomplete": 0,
        "drop_off_never_logged_in": 2,     # buyer_never_logged, buyer_new
        # Registered split by role, current period: buyer_new + pending_buyer
        # are both buyers.
        "registered_current_buyer": 2,
        "registered_current_supplier": 0,
        # Organization-creation → first-ever live order falling inside the
        # period: only the live buyer org (created 5/9 00:00, first live
        # order bid_live_filled 6/4 12:00 → 636 hours). The supplier and
        # returning organizations placed their first live orders before the
        # period, so they are outside this cohort.
        "org_to_first_live_order_sample": 1,
        "org_to_first_live_order_median_hours": 636,
        # Members whose earliest login-day row falls inside the period:
        # supplier_active (6/15), trader_active (6/21). buyer_active's
        # earliest recorded day is 5/20 (previous period). Two members is a
        # segmented small cell, so the stage and its duration distribution
        # suppress.
        "first_recorded_login_in_period": 2,
        "first_login_suppressed": True,
        # Coverage starts at the earliest UserLoginDay activity date.
        "login_history_coverage_start_date": date(2026, 5, 20),
    },
    "orders": {
        # Orders created in the period by recognized non-demo organizations
        # (rule 14): live buyer 3 + live supplier 3 + returning 1 = 7.
        "live_current": 7,
        # ask_live_expired (5/15) only.
        "live_previous": 1,
        "live_bids_current": 4,
        "live_asks_current": 3,
        # Distinct live organizations per side in the period: bids from the
        # live buyer org and the returning org; asks from the supplier only.
        "bid_organizations_current": 2,
        "ask_organizations_current": 1,
        "demo_current": 1,      # bid_demo_open
        "unknown_current": 1,   # bid_unknown_open (pending-only organization)
        # Distinct live organizations creating ≥1 live order in the period.
        "participating_organizations_current": 3,
        "participating_organizations_previous": 1,  # supplier via ask_live_expired
        # Execution: live orders created in period linked (bid or ask) to a
        # CONFIRMED/DELIVERED/PAID trade with economic confirmation < end:
        # bid_live_filled + ask_live_filled (trade paid, confirmed 6/6) and
        # ask_live_partial (trade confirmed 6/12) → 3 of 7. Legacy trades
        # without an order link never enter this rate.
        "execution_rate_numerator": 3,
        "execution_rate_denominator": 7,
    },
    "trades": {
        # Live trades with confirmed_at in [start, end): paid (6/6),
        # confirmed (6/12), paid_missing_paid_at (6/16), and the orderless
        # snapshot (6/18) = 4. Hardened rows no longer need timestamp fallback.
        "confirmed_current_strict": 4,
        "confirmed_current_with_legacy_fallback": 4,
        "legacy_timestamp_fallback_count": 0,
        # delivered_previous confirmed 5/20.
        "confirmed_previous_strict": 1,
        # coalesce(final_quantity_mt, quantity_mt) over the strict set:
        # 400 (final) + 250 + 120 + 80 = 850.
        "confirmed_volume_current_strict_mt": "850",
        "confirmed_volume_current_with_legacy_fallback_mt": "850",
        "confirmed_volume_previous_mt": "290",  # final quantity of delivered_previous
        # Distinct live buyer/seller organizations in the strict confirmed set.
        "trading_organizations_current": 2,
        "trading_organizations_previous": 2,
        # final_total_usd over PAID trades bucketed by paid_at: 316,000 (6/20).
        # paid_missing_paid_at settles in the following period.
        "realized_gmv_current_usd": "316000",
        "missing_paid_at_count": 0,
        # Commission.amount_usd where status=PAID bucketed by payment_date:
        # 1,580 (6/25). The PAID/no-date row is excluded and counted below.
        "realized_revenue_current_usd": "1580",
        "missing_commission_payment_date_count": 1,
        "commission_outstanding_pending_usd": "1012.50",
        "commission_outstanding_invoiced_usd": "1152.75",
        "demo_trades_current": 1,     # exact allowlisted DEMO/DEMO pair
        "unknown_trades_current": 0,
    },
    "retention": {
        # Organizations with a live order or confirmed trade in both periods:
        # live buyer (trade 6/6 · trade 5/20) and live supplier (orders 6/x ·
        # order 5/15 + trade 5/20) = 2. Returning org has no previous-period
        # activity, so it is not retained.
        "retained_organizations": 2,
        # Active current (order 6/21), inactive previous, active earlier
        # (order 3/20): returning org.
        "reactivated_organizations": 1,
        # Previous-period equivalents: nothing was active in both the
        # previous period and the one before it (2026-04-02 → 2026-05-02 has
        # no activity), and nothing reactivated into the previous period.
        "retained_organizations_previous": 0,
        "reactivated_organizations_previous": 0,
        # Returning members need login coverage over BOTH windows; the
        # previous window predates coverage (5/20), so the metric is null.
        "returning_members_default_window": None,
        # Distinct UTC activity days (order created or trade confirmed) per
        # live organization inside the period:
        #   live buyer: orders 6/4, 6/7, 6/9 + trades 6/6, 6/12, 6/16 →
        #     6 days (3+ bucket)
        #   live supplier: orders 6/3, 6/5, 6/8 + trades 6/6, 6/12, 6/16 →
        #     6 days (3+ bucket)
        #   returning: order 6/21 → 1 day
        "repeat_participation_1_day": 1,
        "repeat_participation_2_days": 0,
        "repeat_participation_3_plus_days": 2,
    },
    "liquidity": {
        # As-of PERIOD_END. Eligible live quote: OPEN/PARTIALLY_FILLED,
        # recognized non-demo org, remaining > 0, delivery point set, not
        # expired: bid_live_open, bid_returning_open, ask_live_open (BM/SG),
        # ask_live_partial (BM/RT).
        "eligible_live_quotes": 4,
        "two_sided_slices": 1,   # Bio Methanol / Singapore / SPOT
        "one_sided_slices": 1,   # Bio Methanol / Rotterdam / SPOT (ask only)
        "crossed_slices": 0,
        # BM/SG/SPOT has three distinct live organizations → unsuppressed.
        "bm_sg_spot": {
            "distinct_live_organizations": 3,
            "best_bid_usd_per_mt": "780",
            "best_ask_usd_per_mt": "800",
            "spread_usd_per_mt": "20",
            "mid_usd_per_mt": "790",
            # (spread / mid) × 10,000 — assert via components, not a float.
            "best_bid_depth_mt": "500",
            "best_ask_depth_mt": "1000",
            # Bids priced ≥ 780 × 0.99 = 772.2 → 500 + 200; asks ≤ 808 → 1000.
            "one_percent_bid_depth_mt": "700",
            "one_percent_ask_depth_mt": "1000",
            # Demo depth is reported separately, never merged (rule 6).
            "demo_bid_depth_mt": "100",
        },
        # BM/RT/SPOT: single contributing organization → price/depth/spread
        # suppressed (rule 12) while the slice still counts as one-sided.
        "bm_rt_spot_suppressed": True,
        # Open live orders at as-of (all created 12:00 UTC): ages 25.5d
        # (ask 6/5), 23.5d (bid 6/7), 22.5d (ask 6/8), 9.5d (bid 6/21) →
        # median (22.5 + 23.5) / 2 = 23.0 days.
        "median_open_order_age_hours": 552,
        # Creation→first economic confirmation: ask_live_filled 6/3 12:00 →
        # 6/6 09:00 = 69h; bid_live_filled 6/4 12:00 → 45h; ask_live_partial
        # 6/8 12:00 → 6/12 10:00 = 94h. Median = 69h.
        "median_hours_to_first_fill": 69,
    },
    "engagement": {
        # As-of PERIOD_END (upper bound 7/1): DAU counts 6/30 (no logins),
        # WAU counts [6/24, 7/1) (none — last member login is 6/21), MAU
        # counts [6/1, 7/1): buyer_active, supplier_active, trader_active.
        "dau": 0,
        "wau": 0,
        "mau": 3,
    },
    "behavioral": {
        # From UMAMI_STATS_PAYLOAD: totaltime 1800 / visits 90 = 20.0 —
        # labelled session duration, never active time (§1.5).
        "average_session_duration_seconds": 20.0,
    },
}

# ---------------------------------------------------------------------------
# Canned Umami payloads — existing aggregate contract (MockTransport tests)
# ---------------------------------------------------------------------------

UMAMI_WEBSITE_ID = "11111111-2222-3333-4444-555555555555"

UMAMI_STATS_PAYLOAD = {"visitors": 120, "visits": 90, "pageviews": 300, "totaltime": 1800}

UMAMI_PAGEVIEWS_PAYLOAD = {
    "pageviews": [{"x": "2026-06-10", "y": 40}, {"x": "2026-06-11", "y": 35}],
    "sessions": [{"x": "2026-06-10", "y": 30}, {"x": "2026-06-11", "y": 25}],
}

UMAMI_EVENT_SERIES_PAYLOAD = [
    {"x": "signup_started", "t": "2026-06-10 00:00:00", "y": 6},
    {"x": "signup_started", "t": "2026-06-11 00:00:00", "y": 4},
    {"x": "platform_navigation", "t": "2026-06-10 00:00:00", "y": 25},
    # Unknown names must be dropped from totals and series.
    {"x": "unregistered_event", "t": "2026-06-10 00:00:00", "y": 99},
]

UMAMI_ENTRY_METRICS_PAYLOAD = [{"x": "/signup", "y": 40}, {"x": "/", "y": 22}]

UMAMI_REFERRER_METRICS_PAYLOAD = [
    {"x": "google.com", "y": 25},
    # Empty referrer: rendered as Direct / unknown, distinct from no data.
    {"x": "", "y": 10},
]

EXPECTED_EVENT_TOTALS = {"signup_started": 10, "platform_navigation": 25}

# ---------------------------------------------------------------------------
# Umami 3.2.0 event-data routes (verified against staging on 2026-07-15)
# ---------------------------------------------------------------------------

# Canned rows matching the shapes the smoke validators enforce and the live
# collector returns. The verified contract:
#   properties?startAt&endAt                     → inventory rows below
#   events?startAt&endAt&event=<name>            → per-value rows below
#     (the unfiltered events form returns HTTP 500 on this build)
#   values?startAt&endAt&event=<name>&propertyName=<prop> → value rows below
#     (the filter parameter is ``event``; ``eventName`` is silently ignored)
# If a re-run of the staging smoke disproves a shape, update the validator,
# these rows, and docs/behavioral-analytics-contract.md in the same commit.
UMAMI_EVENT_DATA_CONTRACT: dict[str, list[dict[str, object]]] = {
    "events": [
        {"eventName": "platform_navigation", "propertyName": "destination",
         "dataType": 1, "propertyValue": "marketplace", "total": 18},
        {"eventName": "platform_navigation", "propertyName": "destination",
         "dataType": 1, "propertyValue": "map", "total": 9},
    ],
    "properties": [
        {"eventName": "platform_navigation", "propertyName": "destination", "dataType": 1, "total": 42},
        {"eventName": "market_slice_selected", "propertyName": "product", "dataType": 1, "total": 17},
    ],
    "values": [
        {"value": "marketplace", "total": 18},
        {"value": "map", "total": 9},
    ],
}

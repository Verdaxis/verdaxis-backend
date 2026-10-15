"""The KYC execution gate on/off switch (Private Beta).

The gate is on by default (fail safe). When it is off, the EXECUTION policy
follows the pre-gate advisory rule: only a rejection or a review bound to
another organization revokes access, and an eligible user reports the new
``KYC_NOT_ENFORCED`` reason. Every gate test states both settings.

The route, matching and visibility fixtures live in
``tests.unit.test_kyc_execution_gate``; this module imports them so both
suites exercise the same market harness.
"""
import asyncio
from datetime import UTC, datetime
from decimal import Decimal

import pytest
from fastapi import HTTPException
from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.config import settings
from app.database import Base
from app.middleware.execution import require_execution_eligible_user
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
)
from app.models.subscription import Subscription, SubscriptionTier
from app.models.user import OrganizationProvenance, OrgType, UserRole
from app.routers import orderbook as orderbook_router
from app.routers import trades as trades_router
from app.routers.auth_simple import current_user_profile
from app.schemas.orderbook import OrderCreate, OrderUpdate
from app.services import kyc_execution
from app.services.kyc_execution import (
    ExecutionEligibilityReason,
    customer_organization_is_execution_eligible,
    evaluate_kyc_execution,
)
from app.services.market_data_eligibility import public_order_is_visible
from app.services.matching_engine import match_order
from app.services.order_terms import order_terms_digest
from tests.unit.test_kyc_execution_gate import (
    _expect_kyc_required,
    _market,
    _request,
    _trade,
    _without_kyc,
)
from tests.unit.test_trade_watchlist_hooks import (
    REQUIRED_TABLES,
    _make_ask,
    _make_delivery_point,
    _make_org,
    _make_product,
    _make_user,
)

pytestmark = pytest.mark.asyncio

# The trade confirm path advances referrals, so add that table to the shared
# market harness for this suite.
_TABLES = ["referrals", "notifications", *REQUIRED_TABLES]
_DELETE_ORDER = [
    "referrals",
    "notifications",
    "commissions",
    "rfq_quotes",
    "rfqs",
    "negotiations",
    "match_suggestions",
    "audit_logs",
    "watchlist_events",
    "watchlist_targets",
    "watchlists",
    "market_event_outbox",
    "live_slice_benchmarks",
    "trades",
    "subscriptions",
    "orderbook_orders",
    # kyc_execution_grace references users and organizations, so delete it
    # before them.
    "kyc_execution_grace",
    "users",
    "delivery_points",
    "products",
    "organizations",
]


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[name] for name in _TABLES]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    factory = async_sessionmaker(
        bind=async_engine, class_=AsyncSession, expire_on_commit=False, autoflush=False
    )
    async with factory() as session:
        yield session
        await session.rollback()
        for table in _DELETE_ORDER:
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


@pytest.fixture(params=[True, False], ids=["gate_on", "gate_off"])
def kyc_gate(request, monkeypatch):
    """Run every KYC gate test with the switch on and off."""
    monkeypatch.setattr(settings, "KYC_EXECUTION_GATE_ENABLED", request.param)
    return request.param


async def _pending_buyer(db, market):
    return await _without_kyc(db, market.buyer)


async def _pending_supplier(db, market):
    return await _without_kyc(db, market.supplier)


async def _executable_market(db, monkeypatch):
    """The shared market with a fully specified ASK so it stays executable."""
    market = await _market(db, monkeypatch)
    market.ask.carbon_intensity_gco2_mj = Decimal("18.50")
    market.ask.feedstock = "Waste biomass"
    market.ask.origin = "Singapore"
    await db.commit()
    return market


def _bid(product_id, delivery_point_id, *, org_id, owner_user_id, price="1200", qty="500"):
    return OrderBookOrder(
        organization_id=org_id,
        owner_user_id=owner_user_id,
        provenance=OrganizationProvenance.REAL,
        side=OrderSide.BID,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(qty),
        remaining_quantity_mt=Decimal(qty),
        price_per_mt_usd=Decimal(price),
        availability_window="SPOT",
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
    )


# ---------------------------------------------------------------------------
# The decision point
# ---------------------------------------------------------------------------


async def test_the_setting_defaults_to_enforcing():
    """A missing setting must keep enforcing KYC (fail safe)."""
    from app.config import Settings

    assert Settings.model_fields["KYC_EXECUTION_GATE_ENABLED"].default is True


async def test_profile_reports_the_gate_state(db, kyc_gate):
    organization = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    user = await _make_user(db, organization, UserRole.BUYER)
    user.kyc_status = "PENDING"
    user.kyc_organization_id = None
    await db.commit()

    profile = await current_user_profile(db, user)
    eligibility = profile.execution_eligibility

    assert eligibility.grace_expires_at is None
    if kyc_gate:
        assert eligibility.eligible is False
        assert eligibility.reason == ExecutionEligibilityReason.KYC_REQUIRED
    else:
        assert eligibility.eligible is True
        assert eligibility.reason == ExecutionEligibilityReason.KYC_NOT_ENFORCED
        assert eligibility.reason.value == "KYC_NOT_ENFORCED"


async def test_rejected_still_blocks_in_both_states(db, kyc_gate):
    organization = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    user = await _make_user(db, organization, UserRole.BUYER)
    user.kyc_status = "REJECTED"
    user.kyc_organization_id = organization.id
    await db.commit()

    eligibility = await evaluate_kyc_execution(db, user)

    assert eligibility.eligible is False
    assert eligibility.reason == ExecutionEligibilityReason.KYC_REJECTED


async def test_review_bound_to_another_org_blocks_when_gate_off(db, monkeypatch):
    monkeypatch.setattr(settings, "KYC_EXECUTION_GATE_ENABLED", False)
    organization = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    other = await _make_org(db, "Other", OrgType.SHIPPING_LINE)
    user = await _make_user(db, organization, UserRole.BUYER)
    user.kyc_status = "APPROVED"
    user.kyc_organization_id = other.id
    await db.commit()

    eligibility = await evaluate_kyc_execution(db, user)

    assert eligibility.eligible is False
    assert eligibility.reason == ExecutionEligibilityReason.KYC_REQUIRED


# ---------------------------------------------------------------------------
# Route gates
# ---------------------------------------------------------------------------


async def test_create_order_route_follows_the_gate(db, kyc_gate, monkeypatch):
    market = await _market(db, monkeypatch)
    buyer = await _pending_buyer(db, market)
    call = orderbook_router.create_order(
        request=_request(),
        order_data=OrderCreate(
            side=OrderSide.BID,
            product_id=market.ask.product_id,
            delivery_point_id=market.ask.delivery_point_id,
            quantity_mt=Decimal(500),
            price_per_mt_usd=Decimal(1000),
        ),
        current_user=buyer,
        db=db,
    )
    if kyc_gate:
        await _expect_kyc_required(call)
    else:
        created = await call
        assert created.side == OrderSide.BID


async def test_update_order_route_follows_the_gate(db, kyc_gate, monkeypatch):
    market = await _executable_market(db, monkeypatch)
    supplier = await _pending_supplier(db, market)
    call = orderbook_router.update_order(
        order_id=market.ask.id,
        request=_request(),
        update_data=OrderUpdate(price_per_mt_usd=Decimal(1080)),
        current_user=supplier,
        db=db,
    )
    if kyc_gate:
        await _expect_kyc_required(call)
    else:
        updated = await call
        assert updated.price_per_mt_usd == Decimal(1080)


async def test_inventory_publish_gate_dependency_follows_the_switch(db, kyc_gate, monkeypatch):
    market = await _market(db, monkeypatch)
    supplier = await _pending_supplier(db, market)
    call = require_execution_eligible_user(current_user=supplier, db=db)
    if kyc_gate:
        await _expect_kyc_required(call)
    else:
        assert await call is supplier


async def test_create_trade_route_follows_the_gate(db, kyc_gate, monkeypatch):
    market = await _market(db, monkeypatch)
    buyer = await _pending_buyer(db, market)
    call = trades_router.create_trade(
        payload=trades_router.TradeCreate(
            order_id=market.ask.id,
            quantity_mt=Decimal(100),
            expected_terms_digest=order_terms_digest(market.ask),
        ),
        request=_request(),
        db=db,
        current_user=buyer,
    )
    if kyc_gate:
        await _expect_kyc_required(call)
    else:
        trade = await call
        assert trade.status == trades_router.TradeStatus.PENDING_CONFIRMATION


async def test_confirm_trade_route_follows_the_gate(db, kyc_gate, monkeypatch):
    market = await _market(db, monkeypatch)
    supplier = await _pending_supplier(db, market)
    pending_trade = await _trade(db, market, status=trades_router.TradeStatus.PENDING_CONFIRMATION)
    call = trades_router.confirm_trade(
        trade_id=pending_trade.id, request=_request(), db=db, current_user=supplier
    )
    if kyc_gate:
        await _expect_kyc_required(call)
    else:
        confirmed = await call
        assert confirmed.status == trades_router.TradeStatus.CONFIRMED


async def test_rejected_route_caller_is_blocked_in_both_states(db, kyc_gate, monkeypatch):
    market = await _market(db, monkeypatch)
    market.buyer.kyc_status = "REJECTED"
    market.buyer.kyc_organization_id = market.buyer.organization_id
    await db.commit()

    with pytest.raises(HTTPException) as failure:
        await orderbook_router.create_order(
            request=_request(),
            order_data=OrderCreate(
                side=OrderSide.BID,
                product_id=market.ask.product_id,
                delivery_point_id=market.ask.delivery_point_id,
                quantity_mt=Decimal(500),
                price_per_mt_usd=Decimal(1000),
            ),
            current_user=market.buyer,
            db=db,
        )
    assert failure.value.status_code == 403
    assert failure.value.detail["code"] == "KYC_REQUIRED"
    assert failure.value.detail["kyc_status"] == "REJECTED"


# ---------------------------------------------------------------------------
# Matching and public visibility
# ---------------------------------------------------------------------------


async def _non_kyc_ask(db):
    supplier_org = await _make_org(db, "Supplier", OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    supplier.kyc_status = "PENDING"
    supplier.kyc_organization_id = None
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(
        db,
        org_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        owner_user_id=supplier.id,
    )
    # A Bio Methanol ASK is public and executable only with complete supplier
    # metadata; the shared _make_ask omits the burden fields.
    ask.carbon_intensity_gco2_mj = Decimal("18.50")
    ask.feedstock = "Waste biomass"
    ask.origin = "Singapore"
    await db.commit()
    return SimpleAsk(supplier_org=supplier_org, supplier=supplier, product=product, delivery_point=delivery_point, ask=ask)


class SimpleAsk:
    def __init__(self, **kwargs):
        self.__dict__.update(kwargs)


async def test_matching_includes_a_non_kyc_resting_order_only_when_gate_is_off(
    db, kyc_gate
):
    fixture = await _non_kyc_ask(db)
    buyer_org = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    db.add(
        Subscription(
            org_id=buyer_org.id, tier=SubscriptionTier.STANDARD, started_at=datetime.now(UTC)
        )
    )
    db.add(
        Subscription(
            org_id=fixture.supplier_org.id,
            tier=SubscriptionTier.STANDARD,
            started_at=datetime.now(UTC),
        )
    )
    bid = _bid(
        fixture.product.id,
        fixture.delivery_point.id,
        org_id=buyer_org.id,
        owner_user_id=buyer.id,
    )
    db.add(bid)
    await db.commit()

    trades = await match_order(db, bid)

    if kyc_gate:
        assert trades == []
    else:
        assert len(trades) == 1
        assert trades[0].seller_id == fixture.supplier_org.id


async def test_public_visibility_of_a_non_kyc_order_follows_the_gate(db, kyc_gate):
    fixture = await _non_kyc_ask(db)

    visible = await public_order_is_visible(db, fixture.ask.id)

    assert visible is (not kyc_gate)


# ---------------------------------------------------------------------------
# Assisted execution
# ---------------------------------------------------------------------------


async def test_assisted_customer_org_needs_one_eligible_member(db, kyc_gate):
    organization = await _make_org(db, "Customer", OrgType.FUEL_SUPPLIER)
    member = await _make_user(db, organization, UserRole.SUPPLIER)
    member.kyc_status = "PENDING"
    member.kyc_organization_id = None
    await db.commit()

    eligible = await customer_organization_is_execution_eligible(db, organization.id)

    assert eligible is (not kyc_gate)


# ---------------------------------------------------------------------------
# Grace notice CLI and the startup log line
# ---------------------------------------------------------------------------


async def _no_database_contact():
    raise AssertionError("the disabled gate must refuse before contacting the database")


async def test_grace_notice_cli_refuses_when_gate_is_off(monkeypatch):
    from app.cli import send_kyc_grace_notices as cli

    monkeypatch.setattr(settings, "KYC_EXECUTION_GATE_ENABLED", False)
    monkeypatch.setattr(settings, "ENVIRONMENT", "production")
    monkeypatch.setattr(settings, "RELEASE_SHA", "a" * 40)
    monkeypatch.setattr(cli, "verify_database_runtime", _no_database_contact)

    # main() runs asyncio.run; run it in a worker thread and prove it refuses
    # before any database contact.
    exit_code = await asyncio.to_thread(
        cli.main, ["--environment", "production", "--release-sha", "a" * 40]
    )

    assert exit_code != 0


async def test_grace_notice_cli_proceeds_past_the_gate_when_it_is_on(monkeypatch):
    from app.cli import send_kyc_grace_notices as cli

    monkeypatch.setattr(settings, "KYC_EXECUTION_GATE_ENABLED", True)
    monkeypatch.setattr(settings, "ENVIRONMENT", "production")
    monkeypatch.setattr(settings, "RELEASE_SHA", "a" * 40)
    monkeypatch.setattr(cli, "verify_database_runtime", _no_database_contact)

    with pytest.raises(AssertionError):
        await cli.run_once(expected_environment="production", expected_release_sha="a" * 40)


async def test_lifespan_logs_the_gate_state(monkeypatch, kyc_gate):
    from app import main as main_module

    events = []

    class _Recorder:
        def info(self, message, *args, **kwargs):
            events.append(message)

    async def _noop():
        return None

    class _StubDispatcher:
        def __init__(self, *args, **kwargs):
            pass

        async def start(self):
            return None

        async def stop(self):
            return None

    monkeypatch.setattr(main_module, "logger", _Recorder())
    monkeypatch.setattr("app.database.verify_database_runtime", _noop)
    monkeypatch.setattr(
        "app.services.market_event_dispatch.MarketEventDispatcher", _StubDispatcher
    )

    async with main_module.lifespan(main_module.app):
        pass

    expected = kyc_execution.kyc_execution_gate_startup_message()
    assert expected == "KYC execution gate: " + ("enabled" if kyc_gate else "disabled")
    assert expected in events

"""Query-local JIT controls on an actual disposable PostgreSQL aggregate."""
from datetime import UTC, datetime
from decimal import Decimal

import pytest
from sqlalchemy import Select, func, select, text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.demo_identities import DEMO_SEED_BUYERS, DEMO_SEED_SUPPLIERS
from app.models.orderbook import OrderSide
from app.models.user import Organization, OrganizationProvenance, OrgType
from app.seeds.catalog_seed import DELIVERY_POINT_IDS, PRODUCT_IDS, seed_catalog
from app.services import demo_activity
from app.services.forward_curve_market_slices import (
    SliceKey,
    ViewerContext,
    forward_curve_market_slices,
)


@pytest.fixture
async def managed_market(market_pg, monkeypatch):
    # Twenty generated orders exercise the real admission and aggregate query
    # without duplicating the separate full-book resource benchmark.
    monkeypatch.setattr(demo_activity, "PRICING", {
        "B30": {"Singapore": demo_activity.PRICING["B30"]["Singapore"]},
    })
    monkeypatch.setattr(demo_activity, "activity_windows", lambda _now: ("SPOT",))
    orders = demo_activity.build_demo_market_coverage(datetime.now(UTC))
    assert len(orders) == 20
    expected_bid = max(order.price_per_mt_usd for order in orders if order.side == OrderSide.BID)
    expected_ask = min(order.price_per_mt_usd for order in orders if order.side == OrderSide.ASK)
    factory = async_sessionmaker(market_pg, class_=AsyncSession, expire_on_commit=False, autoflush=False)
    async with factory() as db:
        await seed_catalog(db)
        db.add_all([
            Organization(id=identity, name=name, type=OrgType.FUEL_BUYER,
                         provenance=OrganizationProvenance.DEMO, verification_status="APPROVED")
            for identity, name in DEMO_SEED_BUYERS
        ])
        db.add_all([
            Organization(id=identity, name=name, type=OrgType.FUEL_SUPPLIER,
                         provenance=OrganizationProvenance.DEMO, verification_status="APPROVED")
            for identity, name, _tier in DEMO_SEED_SUPPLIERS
        ])
        await db.flush()
        db.add_all(orders)
        await db.commit()
        groups = await forward_curve_market_slices.load_product_groups(db)
    group = next(group for group in groups if group.market_product == "B30")
    assert group.representative_product_id == PRODUCT_IDS["B30"]
    key = SliceKey("B30", DELIVERY_POINT_IDS["Singapore"], "SPOT")
    return factory, key, {"B30": group}, expected_bid, expected_ask


@pytest.mark.asyncio
@pytest.mark.parametrize("prior_jit", ["on", "off"])
async def test_orderbook_aggregate_restores_jit_and_preserves_cached_query_results(managed_market, monkeypatch, prior_jit):
    factory, key, groups, expected_bid, expected_ask = managed_market
    async with factory() as db:
        baseline_jit = await db.scalar(text("SHOW jit"))
        connection_pid = await db.scalar(select(func.pg_backend_pid()))
        await db.execute(select(func.set_config("jit", prior_jit, True)))
        execute = db.execute
        observed_jit = []

        async def observe_aggregate(statement, *args, **kwargs):
            if isinstance(statement, Select) and "managed_demo_order_count" in statement.selected_columns:
                observed_jit.append(await db.scalar(text("SHOW jit")))
            return await execute(statement, *args, **kwargs)

        monkeypatch.setattr(db, "execute", observe_aggregate)
        first = await forward_curve_market_slices._load_orderbook(db, [key], groups, ViewerContext())
        assert await db.scalar(text("SHOW jit")) == prior_jit
        # Reuse the same physical connection and prepared query.
        second = await forward_curve_market_slices._load_orderbook(db, [key], groups, ViewerContext())
        assert second == first
        assert observed_jit == ["off", "off"]
        assert await db.scalar(text("SHOW jit")) == prior_jit
        bucket = first[key]
        assert bucket["demo_best_bid"] == expected_bid
        assert bucket["demo_best_ask"] == expected_ask
        assert bucket["demo_order_count"] == bucket["managed_demo_order_count"] == 20
        assert bucket["demo_volume_mt"] == Decimal("21000")
        assert bucket["real_order_count"] == bucket["unknown_order_count"] == 0
        await db.rollback()
        assert await db.scalar(text("SHOW jit")) == baseline_jit
    async with factory() as borrowed:
        assert await borrowed.scalar(select(func.pg_backend_pid())) == connection_pid
        assert await borrowed.scalar(text("SHOW jit")) == baseline_jit


@pytest.mark.asyncio
async def test_orderbook_aggregate_sql_error_survives_and_rollback_restores_jit(managed_market, monkeypatch):
    factory, key, groups, _expected_bid, _expected_ask = managed_market
    async with factory() as db:
        baseline_jit = await db.scalar(text("SHOW jit"))
        connection_pid = await db.scalar(select(func.pg_backend_pid()))
        await db.execute(select(func.set_config("jit", "on", True)))
        execute = db.execute
        aggregate_attempts = 0

        async def fail_aggregate(statement, *args, **kwargs):
            nonlocal aggregate_attempts
            if isinstance(statement, Select) and "managed_demo_order_count" in statement.selected_columns:
                aggregate_attempts += 1
                assert await db.scalar(text("SHOW jit")) == "off"
                # An actual server error puts the PostgreSQL transaction into
                # failed state; a restore command here would mask this error.
                return await execute(text("SELECT 1 / 0"))
            return await execute(statement, *args, **kwargs)

        monkeypatch.setattr(db, "execute", fail_aggregate)
        with pytest.raises(DBAPIError) as error:
            await forward_curve_market_slices._load_orderbook(db, [key], groups, ViewerContext())
        assert aggregate_attempts == 1
        assert error.value.orig.sqlstate == "22012"  # division_by_zero, not in_failed_sql_transaction
        await db.rollback()
        assert await db.scalar(text("SHOW jit")) == baseline_jit
    async with factory() as borrowed:
        assert await borrowed.scalar(select(func.pg_backend_pid())) == connection_pid
        assert await borrowed.scalar(text("SHOW jit")) == baseline_jit

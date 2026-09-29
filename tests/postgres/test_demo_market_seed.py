"""Supported demo bootstrap reuses the rolling book on disposable PostgreSQL."""
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import delete, select, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderCreationMethod, OrderSide, Trade
from app.models.seed import SeedRun
from app.models.user import Organization, OrganizationProvenance, OrgType
from app.seeds import market_seed
from app.seeds.catalog_seed import PRODUCT_IDS, DELIVERY_POINT_IDS, seed_catalog
from app.services import demo_activity


@pytest.mark.asyncio
async def test_market_bootstrap_uses_managed_depth_and_preserves_existing_orders_and_history(market_pg, monkeypatch):
    reference = datetime(2026, 10, 1, 12, tzinfo=UTC)

    class FixedDatetime(datetime):
        @classmethod
        def now(cls, tz=None):
            return reference.astimezone(tz) if tz else reference.replace(tzinfo=None)

    monkeypatch.setattr(market_seed, "datetime", FixedDatetime)
    monkeypatch.setenv("ENVIRONMENT", "test")
    # Exercise both new contracts without duplicating the full-book resource test.
    pricing = {
        "B30": {"Singapore": market_seed.PRICING["B30"]["Singapore"]},
        "B100": {"Rotterdam": market_seed.PRICING["B100"]["Rotterdam"]},
    }
    monkeypatch.setattr(market_seed, "PRICING", pricing)
    monkeypatch.setattr(demo_activity, "PRICING", pricing)
    factory = async_sessionmaker(market_pg, class_=AsyncSession, expire_on_commit=False, autoflush=False)
    async with factory() as db:
        await seed_catalog(db)
        await db.execute(delete(SeedRun).where(SeedRun.seed_name == market_seed.MARKET_SEED_NAME))
        # Controlled fixture setup only; normal triggers are active for the seed.
        await db.execute(text("SET LOCAL session_replication_role = replica"))
        organization_id = uuid4()
        db.add(Organization(id=organization_id, name="Existing real buyer", type=OrgType.SHIPPING_LINE,
                            provenance=OrganizationProvenance.REAL, verification_status="APPROVED"))
        existing = OrderBookOrder(
            organization_id=organization_id, provenance=OrganizationProvenance.REAL,
            creation_method=OrderCreationMethod.SELF_SERVICE, side=OrderSide.BID,
            product_id=PRODUCT_IDS["B30"], delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
            quantity_mt=Decimal("1000"), remaining_quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("950"), availability_window="SPOT", status=OrderBookStatus.OPEN,
            expires_at=reference + timedelta(days=1),
        )
        db.add(existing)
        await db.flush()
        await db.execute(text("SET LOCAL session_replication_role = origin"))
        await db.commit()
        existing_id = existing.id

        await market_seed.seed_market_data(db)
        orders = (await db.execute(select(OrderBookOrder))).scalars().all()
        trades = (await db.execute(select(Trade))).scalars().all()
        managed = [row for row in orders if row.idempotency_operation == demo_activity.DEMO_COVERAGE_OPERATION]
        assert len(managed) == 2 * 24 * 20
        slices = {}
        for row in managed:
            key = row.product_id, row.delivery_point_id, row.availability_window, row.side
            slices.setdefault(key, []).append(row)
            assert row.status == OrderBookStatus.OPEN
        assert all(len(rows) == len({row.price_per_mt_usd for row in rows}) == 10 for rows in slices.values())
        assert all(
            row.id == existing_id or row in managed or row.status in (OrderBookStatus.FILLED, OrderBookStatus.EXPIRED)
            for row in orders
        )
        assert len(trades) >= len(market_seed.DEMO_ACCOUNT_TRADE_CONFIGS)
        history_ids = {trade.id for trade in trades}
        linked_order_ids = {identity for trade in trades for identity in (trade.bid_order_id, trade.ask_order_id)}
        assert linked_order_ids <= {row.id for row in orders}
        original_ids = {row.id for row in orders}

        repeat = await demo_activity.ensure_demo_market_coverage(db, now=reference)
        await db.commit()
        assert repeat["coverage_created"] == repeat["coverage_refreshed"] == 0
        await market_seed.seed_market_data(db)  # Seed marker also remains idempotent.
        assert set((await db.execute(select(OrderBookOrder.id))).scalars()) == original_ids
        assert set((await db.execute(select(Trade.id))).scalars()) == history_ids
        await db.refresh(existing)
        assert existing.status == OrderBookStatus.OPEN
        assert existing.price_per_mt_usd == Decimal("950")
        assert existing.remaining_quantity_mt == Decimal("1000")

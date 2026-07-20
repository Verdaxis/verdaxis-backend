"""PostgreSQL-only Sprint 2 integrity checks.

The suite is opt-in and refuses every database that is not explicitly named
with the ``_market_integrity_test`` suffix. It never targets staging or prod.
"""
from __future__ import annotations

import asyncio
import os
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlsplit
from uuid import uuid4
from decimal import Decimal

import pytest
from fastapi import HTTPException
from sqlalchemy import select, text
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.marketplace import FuelType, InventoryItem
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide, Trade
from app.models.user import (
    Organization,
    OrgType,
    OrganizationProvenance,
    User,
    UserRole,
    UserStatus,
)
from app.services.market_locks import acquire_market_slice_lock, acquire_market_slice_locks, market_slice_lock_key
from app.services.inventory_reservations import reserve_inventory, consume_inventory, release_inventory
from app.services.matching_engine import match_order
from app.services.market_invalidation import invalidate_organization_market_access
from app.services.idempotency import acquire_idempotency_lock
from tests.postgres.market_test_support import assign_fixture_real_provenance


def _url() -> str:
    value = os.environ.get("MARKET_INTEGRITY_TEST_DATABASE_URL", "").strip()
    if not value:
        pytest.skip("MARKET_INTEGRITY_TEST_DATABASE_URL is not configured")
    database = urlsplit(value).path.lstrip("/")
    if not database.endswith("_market_integrity_test"):
        raise RuntimeError("refusing a non-disposable market-integrity database")
    return value


@pytest.fixture
def migrated_url() -> str:
    value = _url()
    subprocess.run(
        [sys.executable, "-m", "alembic", "upgrade", "head"],
        check=True,
        cwd=Path(__file__).resolve().parents[2],
        env={**os.environ, "DATABASE_URL": value, "MIGRATOR_DATABASE_URL": value},
    )
    return value


@pytest.fixture
async def pg(migrated_url):
    engine = create_async_engine(migrated_url, pool_size=5, max_overflow=0)
    async with engine.begin() as conn:
        await conn.execute(text("TRUNCATE TABLE trades, orderbook_orders, organizations, products, delivery_points CASCADE"))
    yield engine
    await engine.dispose()


@pytest.mark.asyncio
async def test_bid_and_ask_share_one_postgres_slice_lock(pg):
    product_id = uuid4()
    delivery_point_id = uuid4()
    bid = market_slice_lock_key(
        side=OrderSide.BID,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window="SPOT",
    )
    ask = market_slice_lock_key(
        side=OrderSide.ASK,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window="SPOT",
    )
    assert bid == ask

    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    first_ready = asyncio.Event()
    release_first = asyncio.Event()
    second_started = asyncio.Event()
    second_acquired = asyncio.Event()

    async def first():
        async with factory() as session:
            async with session.begin():
                await acquire_market_slice_lock(
                    session, side=OrderSide.BID, product_id=product_id,
                    delivery_point_id=delivery_point_id, availability_window="SPOT",
                )
                first_ready.set()
                await release_first.wait()

    async def second():
        await first_ready.wait()
        async with factory() as session:
            async with session.begin():
                second_started.set()
                await acquire_market_slice_lock(
                    session, side=OrderSide.ASK, product_id=product_id,
                    delivery_point_id=delivery_point_id, availability_window="SPOT",
                )
                second_acquired.set()

    first_task = asyncio.create_task(first())
    second_task = asyncio.create_task(second())
    await second_started.wait()
    assert not second_acquired.is_set()
    release_first.set()
    await asyncio.gather(first_task, second_task)
    assert second_acquired.is_set()


@pytest.mark.asyncio
async def test_market_slice_lock_timeout_is_bounded_and_rollback_safe(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    product_id = uuid4()
    point_id = uuid4()
    held = asyncio.Event()
    release = asyncio.Event()

    async def holder():
        async with factory() as session:
            async with session.begin():
                await acquire_market_slice_lock(
                    session,
                    side=OrderSide.BID,
                    product_id=product_id,
                    delivery_point_id=point_id,
                    availability_window="SPOT",
                )
                held.set()
                await release.wait()

    holder_task = asyncio.create_task(holder())
    await held.wait()
    try:
        async with factory() as contender:
            started = asyncio.get_running_loop().time()
            with pytest.raises(HTTPException) as raised:
                await acquire_market_slice_lock(
                    contender,
                    side=OrderSide.ASK,
                    product_id=product_id,
                    delivery_point_id=point_id,
                    availability_window="SPOT",
                )
            elapsed = asyncio.get_running_loop().time() - started
            assert raised.value.status_code == 409
            assert raised.value.detail == "Market slice is busy; retry the request"
            assert elapsed < 2
            assert not contender.in_transaction()
    finally:
        release.set()
        await holder_task


@pytest.mark.asyncio
async def test_idempotency_lock_timeout_is_bounded_and_rollback_safe(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    tenant_id = uuid4()
    held = asyncio.Event()
    release = asyncio.Event()

    async def holder():
        async with factory() as session:
            async with session.begin():
                await acquire_idempotency_lock(
                    session,
                    tenant_id=tenant_id,
                    operation="order.create",
                    key="held-key",
                )
                held.set()
                await release.wait()

    holder_task = asyncio.create_task(holder())
    await held.wait()
    try:
        async with factory() as contender:
            started = asyncio.get_running_loop().time()
            with pytest.raises(HTTPException) as raised:
                await acquire_idempotency_lock(
                    contender,
                    tenant_id=tenant_id,
                    operation="order.create",
                    key="held-key",
                )
            elapsed = asyncio.get_running_loop().time() - started
            assert raised.value.status_code == 409
            assert raised.value.detail == "Idempotency key is busy; retry the request"
            assert elapsed < 2
            assert not contender.in_transaction()
    finally:
        release.set()
        await holder_task


@pytest.mark.asyncio
async def test_concurrent_order_retries_are_serialized_by_scoped_unique_key(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as seed:
        org = Organization(name=f"Integrity Org {uuid4()}", type=OrgType.FUEL_BUYER)
        product = Product(name=f"Integrity Product {uuid4()}", fuel_type="Methanol", fuel_grade="Bio", unit="MT")
        point = DeliveryPoint(name=f"Integrity Point {uuid4()}", region="Asia", is_active=True)
        seed.add_all([org, product, point])
        await seed.commit()

    async def attempt():
        async with factory() as session:
            try:
                session.add(OrderBookOrder(
                    organization_id=org.id,
                    provenance=OrganizationProvenance.UNKNOWN,
                    side=OrderSide.BID,
                    product_id=product.id,
                    delivery_point_id=point.id,
                    quantity_mt="100.00",
                    remaining_quantity_mt="100.00",
                    price_per_mt_usd="700.00",
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                    idempotency_key="same-retry-key",
                    idempotency_operation="order.create",
                    idempotency_request_hash="a" * 64,
                ))
                await session.commit()
                return True
            except IntegrityError:
                await session.rollback()
                return False

    results = await asyncio.gather(attempt(), attempt())
    assert sorted(results) == [False, True]


@pytest.mark.asyncio
async def test_reversed_multi_slice_lock_requests_complete_without_deadlock(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    keys = [
        (OrderSide.BID, uuid4(), uuid4(), "2026-Q3"),
        (OrderSide.ASK, uuid4(), uuid4(), "SPOT"),
    ]

    async def acquire(reversed_order: bool):
        async with factory() as session:
            async with session.begin():
                await acquire_market_slice_locks(session, keys[::-1] if reversed_order else keys)

    await asyncio.wait_for(asyncio.gather(acquire(False), acquire(True)), timeout=5)


@pytest.mark.asyncio
async def test_ordinary_app_role_cannot_override_provenance_trigger(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    # Integration note: the market suite runs as the disposable migrator, not
    # a superuser. Role creation and the membership SET LOCAL ROLE needs go
    # through the admin URL; every table grant below stays on the migrator
    # session, which owns the objects. Membership is revoked before the test
    # ends because the runtime migrator safety assertion refuses any role
    # that can SET ROLE, and later fixtures re-run alembic.
    migrator_role = urlsplit(
        os.environ["MARKET_INTEGRITY_TEST_DATABASE_URL"]
    ).username

    async def _admin_execute(*statements: str) -> None:
        admin_engine = create_async_engine(
            os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"],
            isolation_level="AUTOCOMMIT",
        )
        try:
            async with admin_engine.connect() as connection:
                for statement in statements:
                    await connection.execute(text(statement))
        finally:
            await admin_engine.dispose()

    await _admin_execute(
        """
        DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'market_integrity_app_role') THEN
                CREATE ROLE market_integrity_app_role NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE;
            END IF;
        END $$
        """,
        f"GRANT market_integrity_app_role TO {migrator_role}",
    )
    async with factory() as admin:
        org = Organization(name=f"Trigger Org {uuid4()}", type=OrgType.FUEL_BUYER)
        admin.add(org)
        await admin.commit()
        org_id = org.id
        await admin.execute(text("GRANT USAGE ON SCHEMA public TO market_integrity_app_role"))
        await admin.execute(
            text(
                "GRANT SELECT, INSERT, UPDATE ON organizations "
                "TO market_integrity_app_role"
            )
        )
        await admin.execute(
            text(
                "REVOKE ALL ON seed_runs, market_row_quarantines "
                "FROM market_integrity_app_role"
            )
        )
        await admin.commit()

    try:
        async with factory() as session:
            await session.execute(text("SET LOCAL ROLE market_integrity_app_role"))
            org = (await session.execute(select(Organization).where(Organization.id == org_id))).scalar_one()
            org.provenance = OrganizationProvenance.REAL
            with pytest.raises(Exception, match="immutable"):
                await session.commit()
            await session.rollback()

            await session.execute(text("SET LOCAL ROLE market_integrity_app_role"))
            with pytest.raises(Exception, match="operator-only"):
                await session.execute(
                    text(
                        "INSERT INTO organizations (id, name, type, provenance) "
                        "VALUES (:id, 'Forged REAL org', 'FUEL_BUYER', 'REAL')"
                    ),
                    {"id": uuid4()},
                )
            await session.rollback()

            for statement in (
                "INSERT INTO seed_runs (id, seed_name, environment) "
                "VALUES (:id, 'forged', 'test')",
                "INSERT INTO market_row_quarantines "
                "(id, source_table, source_id, original_row, dependencies, "
                "environment, database_name, reason, operator, reference) VALUES "
                "(:id, 'orders', :source_id, '{}', '{}', 'test', "
                "'disposable_test', 'forged', 'app', 'none')",
            ):
                await session.execute(text("SET LOCAL ROLE market_integrity_app_role"))
                with pytest.raises(Exception, match="permission denied"):
                    await session.execute(
                        text(statement), {"id": uuid4(), "source_id": uuid4()}
                    )
                await session.rollback()
            await session.rollback()
            # Even a caller-controlled custom GUC has no effect after hardening.
            await session.execute(text("SET LOCAL ROLE market_integrity_app_role"))
            await session.execute(text("SET LOCAL verdaxis.trusted_provenance_promotion = 'on'"))
            org = (await session.execute(select(Organization).where(Organization.id == org_id))).scalar_one()
            org.provenance = OrganizationProvenance.REAL
            with pytest.raises(Exception, match="immutable"):
                await session.commit()
    finally:
        await _admin_execute(
            f"REVOKE market_integrity_app_role FROM {migrator_role}"
        )


@pytest.mark.asyncio
async def test_postgres_rejects_nonfinite_order_values(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        org = Organization(name=f"Finite Org {uuid4()}", type=OrgType.FUEL_BUYER)
        product = Product(name=f"Finite Product {uuid4()}", fuel_type="Methanol", fuel_grade="Bio", unit="MT")
        point = DeliveryPoint(name=f"Finite Point {uuid4()}", region="Asia", is_active=True)
        session.add_all([org, product, point])
        await session.commit()
        session.add(OrderBookOrder(
            organization_id=org.id,
            provenance=OrganizationProvenance.UNKNOWN,
            side=OrderSide.BID,
            product_id=product.id,
            delivery_point_id=point.id,
            quantity_mt="NaN",
            remaining_quantity_mt="NaN",
            price_per_mt_usd="700.00",
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
        ))
        with pytest.raises(Exception):
            await session.commit()


@pytest.mark.asyncio
async def test_inventory_reservation_lifecycle_conserves_stock_across_publish_fill_decline_cancel_expiry(pg):
    """Every executable lifecycle transition moves the same reservation ledger."""
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        org = Organization(name=f"Inventory Org {uuid4()}", type=OrgType.FUEL_SUPPLIER)
        item = InventoryItem(
            supplier=org,
            fuel_type=FuelType.Methanol,
            product_name="Bio Methanol",
            current_stock_mt="100.00",
            incoming_stock_mt="0.00",
            reserved_stock_mt="0.00",
            price_per_mt_usd="700.00",
        )
        session.add(item)
        await session.flush()
        item_id = item.id

        # publish: current -> reserved; partial fill: reserved is consumed.
        await reserve_inventory(session, item_id, Decimal("100.00"))
        await consume_inventory(session, item_id, Decimal("30.00"))
        # decline/cancel: only the unfilled reservation returns to current.
        await release_inventory(session, item_id, Decimal("20.00"))
        # a quantity increase on the live listing reserves the increment too.
        await reserve_inventory(session, item_id, Decimal("10.00"))
        # expiry releases every remaining reservation.
        await release_inventory(session, item_id, Decimal("60.00"))
        await session.commit()

        refreshed = (await session.execute(select(InventoryItem).where(InventoryItem.id == item_id))).scalar_one()
        assert refreshed.current_stock_mt == Decimal("70.00")
        assert refreshed.reserved_stock_mt == Decimal("0.00")
        assert refreshed.current_stock_mt + refreshed.reserved_stock_mt + Decimal("30.00") == Decimal("100.00")


@pytest.mark.asyncio
async def test_concurrent_direct_slice_move_cannot_escape_locked_identity(pg):
    """A direct updater waits for the row, then the immutable DB guard rejects it."""
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as seed:
        org = Organization(
            name=f"Immutable Slice Org {uuid4()}",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        product = Product(
            name=f"Immutable Slice Product {uuid4()}",
            fuel_type="Methanol",
            fuel_grade="Bio",
            unit="MT",
        )
        point = DeliveryPoint(
            name=f"Immutable Slice Point {uuid4()}", region="Asia", is_active=True
        )
        seed.add_all([org, product, point])
        await seed.flush()
        await assign_fixture_real_provenance(seed, (org,))
        order = OrderBookOrder(
            organization_id=org.id,
            provenance=OrganizationProvenance.REAL,
            side=OrderSide.BID,
            product_id=product.id,
            delivery_point_id=point.id,
            quantity_mt=Decimal("100.00"),
            remaining_quantity_mt=Decimal("100.00"),
            price_per_mt_usd=Decimal("700.00"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
        )
        seed.add(order)
        await seed.commit()
        order_id, product_id, point_id = order.id, product.id, point.id

    row_locked = asyncio.Event()
    release_row = asyncio.Event()
    mover_started = asyncio.Event()

    async def lifecycle_holder():
        async with factory() as session:
            async with session.begin():
                await acquire_market_slice_lock(
                    session,
                    side=OrderSide.BID,
                    product_id=product_id,
                    delivery_point_id=point_id,
                    availability_window="SPOT",
                )
                await session.execute(
                    select(OrderBookOrder)
                    .where(OrderBookOrder.id == order_id)
                    .with_for_update()
                )
                row_locked.set()
                await release_row.wait()

    async def direct_mover():
        await row_locked.wait()
        async with factory() as session:
            with pytest.raises(Exception, match="slice"):
                mover_started.set()
                await session.execute(
                    text(
                        "UPDATE orderbook_orders SET availability_window = '2026-Q4' "
                        "WHERE id = :order_id"
                    ),
                    {"order_id": order_id},
                )
                await session.commit()
            await session.rollback()

    holder = asyncio.create_task(lifecycle_holder())
    mover = asyncio.create_task(direct_mover())
    await mover_started.wait()
    assert not mover.done()
    release_row.set()
    await asyncio.wait_for(asyncio.gather(holder, mover), timeout=5)

    async with factory() as check:
        persisted = await check.get(OrderBookOrder, order_id)
        assert persisted.availability_window == "SPOT"


@pytest.mark.asyncio
async def test_provenance_filter_precedes_candidate_limit(pg):
    """A REAL match remains reachable beyond 101 cheaper UNKNOWN rows."""
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        buyer = Organization(
            name=f"Filtered Buyer {uuid4()}", type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        seller = Organization(
            name=f"Filtered Seller {uuid4()}", type=OrgType.FUEL_SUPPLIER,
            verification_status="APPROVED",
        )
        quarantined = Organization(
            name=f"Filtered Unknown {uuid4()}", type=OrgType.FUEL_SUPPLIER,
            provenance=OrganizationProvenance.UNKNOWN, verification_status="APPROVED",
        )
        product_spec = PRODUCTS_BY_CODE["BIO_METHANOL"]
        point_spec = DELIVERY_POINTS_BY_NAME["Singapore"]
        product = Product(
            id=product_spec.id, name=product_spec.name,
            fuel_type=product_spec.fuel_type, fuel_grade=product_spec.fuel_grade,
            unit=product_spec.unit, is_active=True,
        )
        point = DeliveryPoint(
            id=point_spec.id, name=point_spec.name, region=point_spec.region,
            timezone=point_spec.timezone, is_active=True,
        )
        session.add_all([buyer, seller, quarantined, product, point])
        await session.flush()
        await assign_fixture_real_provenance(session, (buyer, seller))
        # Integrated contract: the matching engine fail-closes on orders
        # without a concrete admitted owner, so both sides carry eligible
        # users.
        buyer_user = User(
            email=f"filtered-buyer-{uuid4()}@integrity.test",
            password_hash="unused",
            role=UserRole.BUYER,
            status=UserStatus.APPROVED,
            organization_id=buyer.id,
            email_verified=True,
            kyc_status="APPROVED",
        )
        seller_user = User(
            email=f"filtered-seller-{uuid4()}@integrity.test",
            password_hash="unused",
            role=UserRole.SUPPLIER,
            status=UserStatus.APPROVED,
            organization_id=seller.id,
            email_verified=True,
            kyc_status="APPROVED",
        )
        session.add_all([buyer_user, seller_user])
        await session.flush()

        def ask(owner, price: str) -> OrderBookOrder:
            return OrderBookOrder(
                organization_id=owner.id,
                owner_user_id=seller_user.id if owner is seller else None,
                provenance=owner.provenance,
                side=OrderSide.ASK,
                product_id=product.id,
                delivery_point_id=point.id,
                quantity_mt=Decimal("1.00"),
                remaining_quantity_mt=Decimal("1.00"),
                price_per_mt_usd=Decimal(price),
                availability_window="SPOT",
                status=OrderBookStatus.OPEN,
                certification_declared=True,
                certification_scheme="ISCC EU",
                specification_standard="IMPCA",
                msds_available=True,
                carbon_intensity_gco2_mj=Decimal("20.00"),
                feedstock="test",
                origin="test",
            )

        unknown_asks = [ask(quarantined, "600.00") for _ in range(101)]
        real_ask = ask(seller, "700.00")
        session.add_all([*unknown_asks, real_ask])
        await session.flush()

        bid = OrderBookOrder(
            organization_id=buyer.id,
            owner_user_id=buyer_user.id,
            provenance=OrganizationProvenance.REAL,
            side=OrderSide.BID,
            product_id=product.id,
            delivery_point_id=point.id,
            quantity_mt=Decimal("1.00"),
            remaining_quantity_mt=Decimal("1.00"),
            price_per_mt_usd=Decimal("700.00"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
        )
        bid.product = product
        bid.delivery_point = point
        session.add(bid)
        await session.flush()
        await acquire_market_slice_lock(
            session,
            side=OrderSide.BID,
            product_id=product.id,
            delivery_point_id=point.id,
            availability_window="SPOT",
        )
        trades = await match_order(session, bid)
        await session.commit()

    assert len(trades) == 1
    assert trades[0].ask_order_id == real_ask.id
    assert trades[0].buyer_provenance == OrganizationProvenance.REAL
    assert trades[0].seller_provenance == OrganizationProvenance.REAL


@pytest.mark.asyncio
async def test_market_revocation_uses_lifecycle_locks_and_conserves_inventory(pg):
    factory = async_sessionmaker(pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        buyer = Organization(
            name=f"Revoked Buyer {uuid4()}",
            type=OrgType.FUEL_BUYER,
            verification_status="APPROVED",
        )
        seller = Organization(
            name=f"Revoked Seller {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
            verification_status="APPROVED",
        )
        product_spec = PRODUCTS_BY_CODE["BIO_METHANOL"]
        point_spec = DELIVERY_POINTS_BY_NAME["Singapore"]
        product = Product(
            id=product_spec.id,
            name=product_spec.name,
            fuel_type=product_spec.fuel_type,
            fuel_grade=product_spec.fuel_grade,
            unit=product_spec.unit,
            is_active=True,
        )
        point = DeliveryPoint(
            id=point_spec.id,
            name=point_spec.name,
            region=point_spec.region,
            timezone=point_spec.timezone,
            is_active=True,
        )
        item = InventoryItem(
            supplier=seller,
            fuel_type=FuelType.Methanol,
            product_name="Bio Methanol",
            current_stock_mt=Decimal("0.00"),
            incoming_stock_mt=Decimal("0.00"),
            reserved_stock_mt=Decimal("100.00"),
            price_per_mt_usd=Decimal("700.00"),
        )
        session.add_all([buyer, seller, product, point, item])
        await session.flush()
        await assign_fixture_real_provenance(session, (buyer, seller))
        ask = OrderBookOrder(
            organization_id=seller.id,
            provenance=OrganizationProvenance.REAL,
            inventory_item_id=item.id,
            side=OrderSide.ASK,
            product_id=product.id,
            delivery_point_id=point.id,
            quantity_mt=Decimal("100.00"),
            remaining_quantity_mt=Decimal("60.00"),
            price_per_mt_usd=Decimal("700.00"),
            availability_window="SPOT",
            status=OrderBookStatus.PARTIALLY_FILLED,
            certification_declared=True,
            certification_scheme="ISCC EU",
            specification_standard="IMPCA",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("20.00"),
            feedstock="biogenic",
            origin="test",
        )
        session.add(ask)
        await session.flush()
        trade = Trade(
            ask_order_id=ask.id,
            buyer_id=buyer.id,
            seller_id=seller.id,
            initiator_org_id=buyer.id,
            buyer_provenance=OrganizationProvenance.REAL,
            seller_provenance=OrganizationProvenance.REAL,
            initiated_by="BUYER",
            quantity_mt=Decimal("40.00"),
            price_per_mt_usd=Decimal("700.00"),
            status="PENDING_CONFIRMATION",
            product_id=product.id,
            product_name=product.name,
            fuel_type=product.fuel_type,
            fuel_grade=product.fuel_grade,
            market_product="BIO_METHANOL",
            delivery_point_id=point.id,
            delivery_point_name=point.name,
            delivery_point_region=point.region,
            availability_window="SPOT",
            market_snapshot_version=1,
        )
        session.add(trade)
        await session.commit()

        events = await invalidate_organization_market_access(
            session,
            organization_id=seller.id,
            actor_user_id=None,
            reason="security approval revoked",
            reference="pg-test",
        )
        seller.verification_status = "REJECTED"
        await session.commit()

        await session.refresh(item)
        await session.refresh(ask)
        await session.refresh(trade)
        assert trade.status.value == "CANCELLED"
        assert ask.status == OrderBookStatus.CANCELLED
        assert ask.remaining_quantity_mt == Decimal("100.00")
        assert item.current_stock_mt == Decimal("100.00")
        assert item.reserved_stock_mt == Decimal("0.00")
        assert events[0].event_type == "market_access_invalidated"
        assert set(events[0].participant_org_ids) == {buyer.id, seller.id}

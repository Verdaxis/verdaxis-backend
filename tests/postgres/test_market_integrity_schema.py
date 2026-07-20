"""Direct PostgreSQL proofs for the market-owned schema contract."""
from __future__ import annotations

from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import Initiator, OrderBookOrder, OrderBookStatus, OrderSide, Trade, TradeStatus
from app.models.user import Organization, OrganizationProvenance, OrgType
from tests.postgres.market_test_support import assign_fixture_real_provenance


@pytest.fixture
async def market_rows(market_pg):
    factory = async_sessionmaker(market_pg, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        buyer = Organization(
            name=f"Schema Buyer {uuid4()}",
            type=OrgType.FUEL_BUYER,
        )
        seller = Organization(
            name=f"Schema Seller {uuid4()}",
            type=OrgType.FUEL_SUPPLIER,
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
        session.add_all([buyer, seller, product, point])
        await session.flush()
        await assign_fixture_real_provenance(session, (buyer, seller))
        order = OrderBookOrder(
            organization_id=seller.id,
            provenance=OrganizationProvenance.REAL,
            side=OrderSide.ASK,
            product_id=product.id,
            delivery_point_id=point.id,
            quantity_mt=Decimal("100.00"),
            remaining_quantity_mt=Decimal("100.00"),
            price_per_mt_usd=Decimal("700.00"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
            certification_declared=True,
            certification_scheme="ISCC EU",
            specification_standard="IMPCA",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("20.00"),
            feedstock="biogenic",
            origin="schema-test",
        )
        session.add(order)
        await session.flush()
        trade = Trade(
            ask_order_id=order.id,
            buyer_id=buyer.id,
            seller_id=seller.id,
            initiator_org_id=buyer.id,
            buyer_provenance=OrganizationProvenance.REAL,
            seller_provenance=OrganizationProvenance.REAL,
            initiated_by=Initiator.BUYER,
            quantity_mt=Decimal("10.00"),
            price_per_mt_usd=Decimal("700.00"),
            status=TradeStatus.CONFIRMED,
            confirmed_at=datetime.now(UTC),
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
        return {
            "buyer": buyer.id,
            "seller": seller.id,
            "product": product.id,
            "point": point.id,
            "order": order.id,
            "trade": trade.id,
        }


async def _rejected(engine, statement: str, **parameters) -> None:
    with pytest.raises(DBAPIError):
        async with engine.begin() as connection:
            await connection.execute(text(statement), parameters)


@pytest.mark.asyncio
async def test_exact_market_constraints_exist(market_pg):
    expected = {
        "ck_organizations_provenance",
        "ck_orderbook_orders_domain",
        "ck_orderbook_orders_numeric_values",
        "ck_orderbook_orders_lifecycle",
        "ck_trades_domain",
        "ck_trades_numeric_values",
        "ck_trades_lifecycle",
        "ck_trades_snapshot",
        "ck_inventory_items_numeric_values",
        "ck_rfqs_domain",
        "ck_rfqs_numeric_values",
        "ck_rfqs_lifecycle",
        "ck_rfq_quotes_domain",
        "ck_rfq_quotes_numeric_values",
        "ck_negotiations_domain",
        "ck_negotiations_numeric_values",
        "ck_negotiations_lifecycle",
        "ck_negotiation_rounds_numeric_values",
    }
    async with market_pg.connect() as connection:
        names = set(
            (
                await connection.execute(
                    text(
                        "SELECT conname FROM pg_constraint "
                        "WHERE connamespace = 'public'::regnamespace AND contype = 'c'"
                    )
                )
            ).scalars()
        )
    assert expected <= names


@pytest.mark.asyncio
async def test_direct_sql_rejects_invalid_provenance_and_snapshot_mutation(market_pg, market_rows):
    await _rejected(
        market_pg,
        "INSERT INTO organizations (id, name, type, provenance) "
        "VALUES (:id, 'Evil provenance', 'FUEL_BUYER', 'EVIL')",
        id=uuid4(),
    )
    await _rejected(
        market_pg,
        "UPDATE organizations SET provenance = 'DEMO' WHERE id = :id",
        id=market_rows["buyer"],
    )
    await _rejected(
        market_pg,
        "UPDATE orderbook_orders SET provenance = 'DEMO' WHERE id = :id",
        id=market_rows["order"],
    )
    await _rejected(
        market_pg,
        "UPDATE orderbook_orders SET availability_window = '2026-Q4' WHERE id = :id",
        id=market_rows["order"],
    )
    await _rejected(
        market_pg,
        "UPDATE orderbook_orders SET product_id = :product WHERE id = :id",
        id=market_rows["order"],
        product=uuid4(),
    )
    await _rejected(
        market_pg,
        "UPDATE trades SET product_name = 'Mutated history' WHERE id = :id",
        id=market_rows["trade"],
    )


_TRADE_INSERT = """
    INSERT INTO trades (
        id, buyer_id, seller_id, initiator_org_id, buyer_provenance,
        seller_provenance, initiated_by, quantity_mt, price_per_mt_usd,
        status, confirmed_at, product_id, product_name, fuel_type,
        fuel_grade, market_product, delivery_point_id, delivery_point_name,
        delivery_point_region, availability_window, market_snapshot_version
    ) VALUES (
        :id, :buyer, :seller, :buyer, 'REAL', 'REAL', 'BUYER', 10, 700,
        'CONFIRMED', now(), :product, :product_name, :fuel_type,
        :fuel_grade, :market_product, :point, :point_name,
        :point_region, 'SPOT', 1
    )
"""


def _valid_snapshot_parameters(market_rows) -> dict:
    return {
        "id": uuid4(),
        "buyer": market_rows["buyer"],
        "seller": market_rows["seller"],
        "product": market_rows["product"],
        "product_name": "Bio Methanol",
        "fuel_type": "Methanol",
        "fuel_grade": "Bio",
        "market_product": "BIO_METHANOL",
        "point": market_rows["point"],
        "point_name": "Singapore",
        "point_region": "Asia",
    }


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("field", "forged_value"),
    [
        ("product_name", "Forged Methanol"),
        ("fuel_type", "Ethanol"),
        ("fuel_grade", "Synthetic"),
        ("market_product", "E_METHANOL"),
        ("point_name", "Amsterdam"),
        ("point_region", "Europe"),
    ],
)
async def test_trade_insert_rejects_each_forged_snapshot_label(
    market_pg, market_rows, field, forged_value
):
    parameters = _valid_snapshot_parameters(market_rows)
    parameters[field] = forged_value
    await _rejected(market_pg, _TRADE_INSERT, **parameters)


@pytest.mark.asyncio
async def test_trade_insert_rejects_inactive_or_noncanonical_market_identity(
    market_pg, market_rows
):
    inactive_product_id, inactive_point_id = uuid4(), uuid4()
    async with market_pg.begin() as connection:
        await connection.execute(
            text(
                "INSERT INTO products "
                "(id, name, fuel_type, fuel_grade, unit, is_active) VALUES "
                "(:id, 'Legacy Methanol', 'Methanol', 'Bio', 'MT', false)"
            ),
            {"id": inactive_product_id},
        )
        await connection.execute(
            text(
                "INSERT INTO delivery_points "
                "(id, name, region, timezone, is_active) VALUES "
                "(:id, 'Amsterdam', 'Europe', 'Europe/Amsterdam', false)"
            ),
            {"id": inactive_point_id},
        )

    bad_product = _valid_snapshot_parameters(market_rows)
    bad_product.update(
        product=inactive_product_id,
        product_name="Legacy Methanol",
    )
    await _rejected(market_pg, _TRADE_INSERT, **bad_product)

    bad_point = _valid_snapshot_parameters(market_rows)
    bad_point.update(
        point=inactive_point_id,
        point_name="Amsterdam",
        point_region="Europe",
    )
    await _rejected(market_pg, _TRADE_INSERT, **bad_point)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "assignment",
    [
        "quantity_mt = 100.001, remaining_quantity_mt = 100.001",
        "quantity_mt = 'NaN', remaining_quantity_mt = 'NaN'",
        "price_per_mt_usd = 'Infinity'",
        "status = 'FILLED', remaining_quantity_mt = 10",
        "status = 'EVIL'",
        "quantity_mt = NULL",
    ],
)
async def test_direct_sql_rejects_order_numeric_domain_and_lifecycle_values(
    market_pg, market_rows, assignment
):
    await _rejected(
        market_pg,
        f"UPDATE orderbook_orders SET {assignment} WHERE id = :id",
        id=market_rows["order"],
    )


@pytest.mark.asyncio
async def test_direct_sql_rejects_unknown_or_mixed_trade_execution(market_pg, market_rows):
    statement = """
        INSERT INTO trades (
            id, buyer_id, seller_id, initiator_org_id, buyer_provenance,
            seller_provenance, initiated_by, quantity_mt, price_per_mt_usd,
            status, confirmed_at, product_id, product_name, fuel_type,
            fuel_grade, market_product, delivery_point_id, delivery_point_name,
            delivery_point_region, availability_window, market_snapshot_version
        ) VALUES (
            :id, :buyer, :seller, :buyer, :buyer_provenance,
            :seller_provenance, 'BUYER', 10, 700, 'CONFIRMED', now(),
            :product, 'Immutable Bio Methanol', 'Methanol', 'Bio',
            'BIO_METHANOL', :point, 'Immutable Point', 'Asia', 'SPOT', 1
        )
    """
    await _rejected(
        market_pg,
        statement,
        id=uuid4(),
        buyer=market_rows["buyer"],
        seller=market_rows["seller"],
        buyer_provenance="UNKNOWN",
        seller_provenance="REAL",
        product=market_rows["product"],
        point=market_rows["point"],
    )

    await _rejected(
        market_pg,
        "INSERT INTO organizations (id, name, type, provenance) VALUES "
        "(:id, 'Unallowlisted Demo Buyer', 'FUEL_BUYER', 'DEMO')",
        id=uuid4(),
    )
    await _rejected(
        market_pg,
        statement,
        id=uuid4(),
        buyer=market_rows["buyer"],
        seller=market_rows["seller"],
        buyer_provenance="REAL",
        seller_provenance="DEMO",
        product=market_rows["product"],
        point=market_rows["point"],
    )


@pytest.mark.asyncio
async def test_direct_sql_rejects_trade_final_lifecycle_errors(market_pg, market_rows):
    await _rejected(
        market_pg,
        "UPDATE trades SET status = 'DELIVERED', delivered_at = now() WHERE id = :id",
        id=market_rows["trade"],
    )
    await _rejected(
        market_pg,
        "UPDATE trades SET quantity_mt = 10.001 WHERE id = :id",
        id=market_rows["trade"],
    )


@pytest.mark.asyncio
async def test_rfq_quote_and_negotiation_domains_fail_closed(market_pg, market_rows):
    rfq_id = uuid4()
    async with market_pg.begin() as connection:
        await connection.execute(
            text(
                "INSERT INTO rfqs (id, buyer_org_id, product_id, delivery_point_id, "
                "quantity_mt, availability_window, is_anonymous, status, expires_at) "
                "VALUES (:id, :buyer, :product, :point, 100, 'SPOT', false, 'OPEN', :expires)"
            ),
            {
                "id": rfq_id,
                "buyer": market_rows["buyer"],
                "product": market_rows["product"],
                "point": market_rows["point"],
                "expires": datetime.now(UTC) + timedelta(days=1),
            },
        )

    await _rejected(
        market_pg,
        "INSERT INTO rfq_quotes (id, rfq_id, seller_org_id, price_per_mt_usd, status) "
        "VALUES (:id, :rfq, :buyer, 700, 'PENDING')",
        id=uuid4(),
        rfq=rfq_id,
        buyer=market_rows["buyer"],
    )
    await _rejected(
        market_pg,
        "UPDATE rfqs SET status = 'ACCEPTED' WHERE id = :id",
        id=rfq_id,
    )
    await _rejected(
        market_pg,
        "UPDATE rfqs SET quantity_mt = 100.001 WHERE id = :id",
        id=rfq_id,
    )
    await _rejected(
        market_pg,
        "UPDATE rfqs SET target_price_per_mt = 'NaN' WHERE id = :id",
        id=rfq_id,
    )
    await _rejected(
        market_pg,
        "INSERT INTO negotiations (id, initiator_org_id, counterparty_org_id, "
        "initiator_side, product_id, delivery_point_id, availability_window, "
        "quantity_mt, current_price, status, last_actor_org_id, expires_at) "
        "VALUES (:id, :buyer, :buyer, 'BUYER', :product, :point, 'SPOT', "
        "100, 700, 'OPEN', :buyer, :expires)",
        id=uuid4(),
        buyer=market_rows["buyer"],
        product=market_rows["product"],
        point=market_rows["point"],
        expires=datetime.now(UTC) + timedelta(days=1),
    )

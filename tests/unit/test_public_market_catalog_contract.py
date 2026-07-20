"""Contract tests for active-catalog public market surfaces."""
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest

from app.routers.availability import get_fuel_availability
from app.market_catalog import (
    CANONICAL_DELIVERY_POINTS,
    CANONICAL_PRODUCTS,
    DELIVERY_POINT_IDS,
    PRODUCT_IDS,
)
from app.models.catalog import Product
from app.seeds.catalog_seed import seed_catalog


def test_market_catalog_is_one_exact_four_by_eight_identity_contract():
    assert len(CANONICAL_PRODUCTS) == 4
    assert len(CANONICAL_DELIVERY_POINTS) == 8
    assert set(PRODUCT_IDS) == {
        "BIO_METHANOL",
        "E_METHANOL",
        "BIO_ETHANOL",
        "SYNTHETIC_ETHANOL",
    }
    assert set(DELIVERY_POINT_IDS) == {
        "Dalian",
        "Busan",
        "Shanghai",
        "Singapore",
        "Rotterdam",
        "Houston",
        "Los Angeles",
        "Santos",
    }


@pytest.mark.asyncio
async def test_availability_query_requires_active_catalog_product_and_delivery_point():
    db = AsyncMock()
    result = MagicMock()
    result.all.return_value = [
        SimpleNamespace(
            port_id="sg-sin",
            port_name="Singapore",
            lat=1.25,
            lng=103.8,
            product_name="Bio Methanol",
            product_fuel_type="Methanol",
            fuel_grade="Bio",
            market_product="BIO_METHANOL",
            real_stock=Decimal("1200.00"),
            demo_stock=Decimal("900.00"),
            real_supplier_count=2,
            demo_supplier_count=3,
            unknown_count=4,
            real_avg_price=Decimal("710.00"),
            demo_avg_price=Decimal("1.00"),
        )
    ]
    db.execute.return_value = result

    response = await get_fuel_availability(fuel_type=None, db=db)

    statement = db.execute.await_args.args[0]
    sql = str(statement.compile(compile_kwargs={"literal_binds": True})).lower()
    assert "products" in sql
    assert "delivery_points" in sql
    assert "products.is_active is true" in sql
    assert "delivery_points.is_active is true" in sql
    assert "organizations" in sql
    assert "users" in sql
    assert "left outer join users" in sql
    assert "coalesce" in sql
    assert str(PRODUCT_IDS["BIO_METHANOL"]).replace("-", "") in sql
    assert str(DELIVERY_POINT_IDS["Singapore"]).replace("-", "") in sql
    assert len(response) == 1
    assert response[0].port_name == "Singapore"
    assert response[0].fuel_type == "BIO_METHANOL"
    assert response[0].market_product_code == "BIO_METHANOL"
    assert response[0].total_stock_mt == Decimal("1200.00")
    assert response[0].source_kind == "LIVE_INVENTORY"
    assert response[0].demo_status == "REAL_ONLY"
    assert response[0].scope == "DELIVERY_POINT"
    assert response[0].unknown_count == 4


@pytest.mark.asyncio
async def test_availability_drops_noncanonical_active_product_instead_of_fallback_label():
    db = AsyncMock()
    result = MagicMock()
    result.all.return_value = [
        SimpleNamespace(
            port_id="sg-sin",
            port_name="Singapore",
            lat=1.25,
            lng=103.8,
            product_name="Generic Methanol",
            product_fuel_type="Methanol",
            fuel_grade="",
            market_product=None,
            real_stock=Decimal("1200.00"),
            demo_stock=Decimal("0"),
            real_supplier_count=2,
            demo_supplier_count=0,
            unknown_count=0,
            real_avg_price=Decimal("710.00"),
            demo_avg_price=None,
        )
    ]
    db.execute.return_value = result

    response = await get_fuel_availability(fuel_type=None, db=db)

    assert response == []


@pytest.mark.asyncio
async def test_availability_prefers_real_and_never_blends_demo_economics():
    db = AsyncMock()
    result = MagicMock()
    result.all.return_value = [
        SimpleNamespace(
            port_id="sg-sin",
            port_name="Singapore",
            lat=1.25,
            lng=103.8,
            product_name="Bio Methanol",
            product_fuel_type="Methanol",
            fuel_grade="Bio",
            market_product="BIO_METHANOL",
            real_stock=Decimal("10.00"),
            demo_stock=Decimal("9999.00"),
            real_supplier_count=1,
            demo_supplier_count=9,
            unknown_count=2,
            real_avg_price=Decimal("700.00"),
            demo_avg_price=Decimal("1.00"),
        )
    ]
    db.execute.return_value = result

    response = await get_fuel_availability(fuel_type=None, db=db)

    assert response[0].total_stock_mt == Decimal("10.00")
    assert response[0].supplier_count == 1
    assert response[0].avg_price_per_mt == Decimal("700.00")
    assert response[0].demo_status == "REAL_ONLY"
    assert response[0].unknown_count == 2


@pytest.mark.asyncio
async def test_catalog_seed_deactivates_every_noncanonical_product_id():
    stale = Product(
        name="Methanol Green",
        fuel_type="Methanol",
        fuel_grade="Green",
        is_active=True,
    )
    product_result = MagicMock()
    product_result.scalars.return_value.all.return_value = [stale]
    point_result = MagicMock()
    point_result.scalars.return_value.all.return_value = []
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.side_effect = [product_result, point_result]

    await seed_catalog(db)

    assert stale.is_active is False
    db.commit.assert_awaited_once()

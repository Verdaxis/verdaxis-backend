"""Reseller rejection through HTTP with the restricted PostgreSQL app role."""
from copy import deepcopy

import pytest

from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.catalog import DeliveryPoint, Product
from tests.postgres.test_fame_rfq import _headers
from tests.postgres.test_fame_rfq import fame_market as fame_market
from tests.unit.test_lng_order import ASK
from tests.unit.test_lng_reseller_admission import RESELLER_DETAIL


@pytest.mark.asyncio
async def test_reseller_create_and_amend_leave_physical_ask_unchanged(fame_market):
    client, seeded = fame_market
    product = PRODUCTS_BY_CODE["BIO_LNG"]
    point = DELIVERY_POINTS_BY_NAME["Rotterdam"]
    async with seeded["owner_factory"]() as session:
        session.add(Product(
            id=product.id, name=product.name, fuel_type=product.fuel_type,
            fuel_grade=product.fuel_grade, unit=product.unit, is_active=True,
        ))
        session.add(DeliveryPoint(
            id=point.id, name=point.name, region=point.region,
            timezone=point.timezone, is_active=True,
        ))
        await session.commit()
    payload = {
        "side": "ASK", "product_id": str(product.id), "delivery_point_id": str(point.id),
        "quantity_mt": "100", "price_per_mt_usd": "1000", "availability_window": "SPOT",
        "msds_available": True, "lng_terms": deepcopy(ASK),
    }
    headers = _headers(seeded["seller_id"])
    rejected = await client.post("/api/orderbook", json=payload, headers=headers)
    assert rejected.status_code == 422, rejected.text
    assert rejected.json() == {"detail": RESELLER_DETAIL}
    payload["lng_terms"]["seller_is_physical_supplier"] = True
    created = await client.post("/api/orderbook", json=payload, headers=headers)
    assert created.status_code == 201, created.text
    rejected = await client.put(
        f"/api/orderbook/{created.json()['id']}", headers=headers,
        json={"lng_terms": ASK, "price_per_mt_usd": "900"},
    )
    assert rejected.status_code == 422, rejected.text
    assert rejected.json() == {"detail": RESELLER_DETAIL}
    mine = await client.get("/api/orderbook/my", headers=headers)
    assert mine.status_code == 200, mine.text
    assert len(mine.json()) == 1
    assert mine.json()[0]["lng_terms"]["seller_is_physical_supplier"] is True
    assert float(mine.json()[0]["price_per_mt_usd"]) == 1000

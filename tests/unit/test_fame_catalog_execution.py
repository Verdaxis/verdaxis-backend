"""UCOME RFQ catalog identity must never become executable liquidity."""
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException

from app.market_catalog import (
    APPROVED_MARKET_PRODUCTS,
    DELIVERY_POINT_IDS,
    PRODUCTS_BY_CODE,
    PRODUCT_IDS,
)
from app.models.catalog import Product, DeliveryPoint
from app.models.orderbook import OrderSide
from app.models.user import UserRole
from app.routers.orderbook import create_order
from app.routers.market_support import _load_catalog
from app.schemas.catalog import ProductResponse
from app.schemas.orderbook import OrderCreate
from app.services.execution_policy import order_is_execution_qualified, orders_execution_compatible
from app.services.market_catalog_validation import require_canonical_market_slice
from app.seeds.forward_monitoring_seed import _PRODUCT_TO_MARKET_PRODUCT


def _product(code="UCOME_B100"):
    spec = PRODUCTS_BY_CODE[code]
    return Product(
        id=spec.id, name=spec.name, fuel_type=spec.fuel_type,
        fuel_grade=spec.fuel_grade, unit=spec.unit,
        min_lot_size=spec.min_lot_size, spec_description=spec.spec_description,
        is_active=True,
    )


def _result(value):
    result = MagicMock()
    result.scalar_one_or_none.return_value = value
    result.scalars.return_value.first.return_value = value
    return result


def test_catalog_reports_rfq_policy_without_changing_alcohol_execution():
    fame = ProductResponse.model_validate(_product())
    assert fame.market_product == "UCOME_B100"
    assert fame.execution_mode == "RFQ_ONLY"
    assert fame.available_delivery_point_ids == [DELIVERY_POINT_IDS["Singapore"]]
    assert fame.min_lot_size == Decimal("1")
    assert "platform input minimum" in fame.spec_description
    alcohol = ProductResponse.model_validate(_product("BIO_METHANOL"))
    assert alcohol.execution_mode == "ORDERBOOK"
    assert len(alcohol.available_delivery_point_ids) == 8
    assert alcohol.min_lot_size == Decimal("200")


def test_fame_is_not_executable_or_seeded_price_evidence():
    assert "UCOME_B100" not in APPROVED_MARKET_PRODUCTS
    assert "UCOME_B100" not in _PRODUCT_TO_MARKET_PRODUCT.values()
    for side in (OrderSide.BID, OrderSide.ASK):
        order = SimpleNamespace(
            product_id=PRODUCT_IDS["UCOME_B100"], side=side,
            certification_scheme="ISCC EU", certification_declared=True,
            off_spec=False,
        )
        assert not order_is_execution_qualified(order)
        assert not orders_execution_compatible(order, order)
    alcohol = SimpleNamespace(
        product_id=PRODUCT_IDS["BIO_METHANOL"], side=OrderSide.ASK,
        certification_scheme="ISCC EU", certification_declared=True,
        off_spec=False,
    )
    assert order_is_execution_qualified(alcohol)


@pytest.mark.asyncio
async def test_fame_canonical_rfq_allows_singapore_and_refuses_other_lanes():
    for lane in ("Singapore", "Rotterdam"):
        product = _product()
        point = DeliveryPoint(id=DELIVERY_POINT_IDS[lane], name=lane)
        db = AsyncMock()
        db.execute.side_effect = [_result(product), _result(point)]
        if lane == "Singapore":
            actual = await require_canonical_market_slice(
                db, product_id=product.id, delivery_point_id=point.id,
            )
            assert actual == (product, point)
        else:
            with pytest.raises(HTTPException, match="Singapore RFQs only"):
                await require_canonical_market_slice(
                    db, product_id=product.id, delivery_point_id=point.id,
                )


@pytest.mark.asyncio
async def test_order_entry_refuses_fame_before_inserting_an_order():
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.return_value = _result(_product())
    user = MagicMock(id=uuid4(), role=UserRole.BUYER, organization_id=uuid4())
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))
    with pytest.raises(HTTPException, match="RFQ-only") as rejected:
        await create_order(
            request=request,
            current_user=user,
            db=db,
            order_data=OrderCreate(
                side=OrderSide.BID, product_id=PRODUCT_IDS["UCOME_B100"],
                delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
                quantity_mt=Decimal("200"), price_per_mt_usd=Decimal("1000"),
            ),
        )
    assert rejected.value.status_code == 400
    db.add.assert_not_called()
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
async def test_assisted_order_catalog_refuses_fame():
    db = AsyncMock()
    db.execute.side_effect = [_result(_product()), _result(DeliveryPoint(
        id=DELIVERY_POINT_IDS["Singapore"], name="Singapore",
    ))]
    with pytest.raises(HTTPException, match="RFQ-only"):
        await _load_catalog(db, PRODUCT_IDS["UCOME_B100"], DELIVERY_POINT_IDS["Singapore"])


@pytest.mark.asyncio
async def test_inventory_publication_cannot_turn_fame_into_an_ask(monkeypatch):
    from app.routers.inventory import publish_inventory_item

    db = AsyncMock()
    db.add = MagicMock()
    user = MagicMock(id=uuid4(), role=UserRole.SUPPLIER, organization_id=uuid4())
    item = SimpleNamespace(id=uuid4(), product_name="UCOME B100", price_per_mt_usd=Decimal("1000"))
    monkeypatch.setattr("app.routers.inventory.execution_party_is_eligible", AsyncMock(return_value=True))
    db.execute.side_effect = [_result(user), _result(SimpleNamespace(id=user.organization_id)),
                              _result(item), _result(None), _result(_product())]
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))
    with pytest.raises(HTTPException, match="RFQ-only"):
        await publish_inventory_item(item_id=item.id, request=request, db=db, current_user=user)
    db.add.assert_not_called()
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
async def test_matching_engine_does_not_match_a_fame_order(monkeypatch):
    from app.models.user import OrganizationProvenance
    from app.services.matching_engine import match_order

    monkeypatch.setattr("app.services.matching_engine.acquire_market_slice_lock", AsyncMock())
    order = SimpleNamespace(
        product_id=PRODUCT_IDS["UCOME_B100"], side=OrderSide.BID,
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"], availability_window="SPOT",
        remaining_quantity_mt=Decimal("200"), expires_at=None,
        provenance=OrganizationProvenance.REAL,
    )
    db = AsyncMock()
    assert await match_order(db, order) == []
    db.execute.assert_not_awaited()

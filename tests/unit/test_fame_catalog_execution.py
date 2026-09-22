"""UCOME joins the shared book with declared terms and no invented seed liquidity."""
from datetime import UTC, datetime
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
from app.seeds.forward_monitoring_seed import _demo_slices
from app.seeds.market_seed import PRICING, CI_DATA


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


def test_catalog_reports_shared_orderbook_without_changing_alcohol_identity():
    fame = ProductResponse.model_validate(_product())
    assert fame.market_product == "UCOME_B100"
    assert fame.execution_mode == "ORDERBOOK"
    assert fame.available_delivery_point_ids == [DELIVERY_POINT_IDS["Singapore"]]
    assert fame.min_lot_size == Decimal("1")
    assert "platform input minimum" in fame.spec_description
    alcohol = ProductResponse.model_validate(_product("BIO_METHANOL"))
    assert alcohol.execution_mode == "ORDERBOOK"
    assert len(alcohol.available_delivery_point_ids) == 8
    assert alcohol.min_lot_size == Decimal("200")


def test_fame_requires_terms_and_has_no_synthetic_seed_evidence():
    assert "UCOME_B100" in APPROVED_MARKET_PRODUCTS
    assert "UCOME B100" not in PRICING
    assert "UCOME B100" not in CI_DATA
    assert all(name != "UCOME B100" for name, _port, _window in _demo_slices(datetime.now(UTC)))
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
async def test_fame_canonical_market_allows_singapore_and_refuses_other_lanes():
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
            with pytest.raises(HTTPException, match="Singapore"):
                await require_canonical_market_slice(
                    db, product_id=product.id, delivery_point_id=point.id,
                )


@pytest.mark.asyncio
async def test_order_entry_requires_b100_terms_before_inserting_an_order():
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.return_value = _result(_product())
    user = MagicMock(id=uuid4(), role=UserRole.BUYER, organization_id=uuid4())
    request = SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))
    with pytest.raises(HTTPException, match="B100 requirements or declarations") as rejected:
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
    assert rejected.value.status_code == 422
    db.add.assert_not_called()
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
async def test_assisted_catalog_uses_the_same_b100_singapore_lane():
    product = _product()
    point = DeliveryPoint(id=DELIVERY_POINT_IDS["Singapore"], name="Singapore")
    db = AsyncMock()
    db.execute.side_effect = [_result(product), _result(point)]
    assert await _load_catalog(db, product.id, point.id) == (product, point)


@pytest.mark.asyncio
async def test_matching_engine_does_not_match_b100_without_structured_terms(monkeypatch):
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

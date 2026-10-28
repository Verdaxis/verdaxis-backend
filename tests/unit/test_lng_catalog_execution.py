"""Bio-LNG uses the shared book only at Rotterdam, without synthetic prices."""
from datetime import UTC, datetime
from decimal import Decimal
from types import SimpleNamespace

import pytest

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS, PRODUCTS_BY_CODE
from app.models.orderbook import OrderSide
from app.schemas.catalog import ProductResponse
from app.services.execution_policy import order_is_execution_qualified, orders_execution_compatible
from tests.unit.test_fame_catalog_execution import _product
from tests.unit.test_lng_order import ASK, BID


def test_catalog_lane_and_no_demo_prices():
    from app.seeds.market_seed import PRICING, CI_DATA
    from app.seeds.forward_monitoring_seed import _demo_slices
    from app.services.public_order_scope import APPROVED_MARKETPLACE_FUEL_TYPES
    product = ProductResponse.model_validate(_product("BIO_LNG"))
    assert product.available_delivery_point_ids == [DELIVERY_POINT_IDS["Rotterdam"]]
    assert product.min_lot_size == Decimal("1")
    assert product.execution_mode == "ORDERBOOK"
    assert "LNG" in APPROVED_MARKETPLACE_FUEL_TYPES
    assert "Bio-LNG" not in PRICING and "Bio-LNG" not in CI_DATA
    assert all(name != "Bio-LNG" for name, _, _ in _demo_slices(datetime.now(UTC)))


def test_execution_requires_terms_lane_and_all_compatibility_rules():
    def order(side, terms, lane="Rotterdam"):
        return SimpleNamespace(
            product_id=PRODUCT_IDS["BIO_LNG"], delivery_point_id=DELIVERY_POINT_IDS[lane],
            side=side, lng_terms=terms, off_spec=False, msds_available=True,
            certification_declared=True, certification_scheme="ISCC EU",
        )
    bid = order(OrderSide.BID, BID)
    ask = order(OrderSide.ASK, ASK)
    assert orders_execution_compatible(bid, ask)
    assert not orders_execution_compatible(order(OrderSide.BID, {**BID, "accept_resellers": False}), ask)
    assert not order_is_execution_qualified(order(OrderSide.ASK, None))
    assert not order_is_execution_qualified(order(OrderSide.ASK, ASK, "Singapore"))


def test_ci_pricing_never_prices_lng_wtt_as_lifecycle_ci():
    from app.services.ci_pricing import calculate_ci_adjusted_price
    assert calculate_ci_adjusted_price(Decimal("1000"), Decimal("0"), Decimal("50"), market_product="BIO_LNG") is None


@pytest.mark.asyncio
async def test_create_rejects_lng_outside_rotterdam():
    from unittest.mock import AsyncMock, MagicMock
    from uuid import uuid4
    from fastapi import HTTPException
    from app.models.user import UserRole
    from app.routers.orderbook import create_order
    from app.schemas.orderbook import OrderCreate
    from tests.unit.test_fame_catalog_execution import _result
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.return_value = _result(_product("BIO_LNG"))
    user = MagicMock(id=uuid4(), role=UserRole.BUYER, organization_id=uuid4())
    user.kyc_status = "APPROVED"
    user.kyc_organization_id = user.organization_id
    with pytest.raises(HTTPException, match="not available at the selected delivery point"):
        await create_order(
            request=SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1")),
            current_user=user, db=db,
            order_data=OrderCreate(side="BID", product_id=PRODUCT_IDS["BIO_LNG"],
                delivery_point_id=DELIVERY_POINT_IDS["Singapore"], quantity_mt=1,
                price_per_mt_usd=1000, lng_terms=BID),
        )
    db.add.assert_not_called()


@pytest.mark.asyncio
async def test_create_rejects_lng_without_terms_before_insert():
    from unittest.mock import AsyncMock, MagicMock
    from uuid import uuid4
    from fastapi import HTTPException
    from app.models.user import UserRole
    from app.routers.orderbook import create_order
    from app.schemas.orderbook import OrderCreate
    db = AsyncMock()
    db.add = MagicMock()
    user = MagicMock(id=uuid4(), role=UserRole.BUYER, organization_id=uuid4())
    user.kyc_status = "APPROVED"
    user.kyc_organization_id = user.organization_id
    with pytest.raises(HTTPException, match="Bio-LNG requirements"):
        await create_order(
            request=SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1")),
            current_user=user, db=db,
            order_data=OrderCreate(side="BID", product_id=PRODUCT_IDS["BIO_LNG"],
                delivery_point_id=DELIVERY_POINT_IDS["Rotterdam"], quantity_mt=1, price_per_mt_usd=1000),
        )
    db.add.assert_not_called()

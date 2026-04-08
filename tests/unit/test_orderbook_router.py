"""Unit tests for orderbook router guardrails."""
import pytest
from decimal import Decimal
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

from fastapi import HTTPException

from app.models.user import UserRole
from app.routers.orderbook import create_order
from app.schemas.orderbook import OrderCreate, OrderSide


def _make_buyer_user():
    user = MagicMock()
    user.role = UserRole.BUYER
    user.organization_id = uuid4()
    return user


class TestCreateOrder:
    @pytest.mark.asyncio
    async def test_bid_rejects_supplier_metadata(self):
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            certification_declared=True,
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                order_data=order,
                current_user=_make_buyer_user(),
                db=AsyncMock(),
            )

        assert exc_info.value.status_code == 400
        assert "certification_declared" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_bid_rejects_explicit_false_supplier_metadata(self):
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            off_spec=False,
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                order_data=order,
                current_user=_make_buyer_user(),
                db=AsyncMock(),
            )

        assert exc_info.value.status_code == 400
        assert "off_spec" in exc_info.value.detail

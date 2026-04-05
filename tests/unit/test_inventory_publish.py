"""Unit tests for publishing inventory into the orderbook."""
import pytest
from unittest.mock import AsyncMock, MagicMock
from decimal import Decimal
from uuid import uuid4

from app.routers.inventory import publish_inventory_item
from app.models.marketplace import FuelType as ModelFuelType
from app.models.user import UserRole


def _make_user():
    user = MagicMock()
    user.role = UserRole.SUPPLIER
    user.organization_id = uuid4()
    return user


def _make_inventory_item():
    item = MagicMock()
    item.id = uuid4()
    item.supplier_id = uuid4()
    item.port_id = "SGSIN"
    item.port = MagicMock()
    item.port.name = "Singapore"
    item.fuel_type = ModelFuelType.Biofuel
    item.product_name = "Test Sustainable Fuel"
    item.current_stock_mt = Decimal("1000")
    item.price_per_mt_usd = Decimal("850")
    item.is_certified = True
    return item


class TestPublishInventoryItem:
    @pytest.mark.asyncio
    async def test_publish_maps_catalog_product_and_delivery_point(self):
        user = _make_user()
        item = _make_inventory_item()

        mock_db = AsyncMock()

        item_result = MagicMock()
        item_result.scalar_one_or_none.return_value = item

        exact_product_result = MagicMock()
        exact_product_result.scalar_one_or_none.return_value = None

        fallback_product = MagicMock()
        fallback_product.id = uuid4()
        fallback_product.name = "Biofuel Bio"
        fallback_product.fuel_type = "Biofuel"
        fallback_product.fuel_grade = "Bio"
        product_result = MagicMock()
        product_result.scalars.return_value.all.return_value = [fallback_product]

        delivery_point = MagicMock()
        delivery_point.id = uuid4()
        delivery_point.name = "Singapore"
        dp_result = MagicMock()
        dp_result.scalar_one_or_none.return_value = delivery_point

        mock_db.execute.side_effect = [
            item_result,
            exact_product_result,
            product_result,
            dp_result,
        ]

        result = await publish_inventory_item(
            item_id=item.id,
            db=mock_db,
            current_user=user,
        )

        assert result["status"] == "published"
        listing = mock_db.add.call_args.args[0]
        assert listing.product_id == fallback_product.id
        assert listing.delivery_point_id == delivery_point.id
        assert listing.port_id == item.port_id
        assert listing.side.value == "ASK"
        assert listing.quantity_mt == Decimal("1000")
        assert listing.remaining_quantity_mt == Decimal("1000")
        assert listing.price_per_mt_usd == Decimal("850")
        mock_db.commit.assert_awaited_once()
        mock_db.refresh.assert_awaited_once()

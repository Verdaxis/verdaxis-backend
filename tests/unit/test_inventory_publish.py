"""Unit tests for inventory publishing helpers."""
import pytest
from unittest.mock import AsyncMock, MagicMock
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

from app.routers.inventory import _listing_payload, publish_inventory_item
from app.models.marketplace import FuelType as ModelFuelType
from app.models.orderbook import OrderBookStatus
from app.models.user import UserRole, UserStatus


def _make_user():
    user = MagicMock()
    user.id = uuid4()
    user.role = UserRole.SUPPLIER
    user.organization_id = uuid4()
    user.status = UserStatus.APPROVED
    user.email_verified = True
    user.must_change_password = False
    user.kyc_status = "APPROVED"
    user.kyc_organization_id = user.organization_id
    user.kyc_external_evidence_reference = "external-case-123"
    user.kyc_review_note = "Externally retained evidence reviewed."
    user.kyc_reviewed_by = uuid4()
    user.kyc_reviewed_at = SimpleNamespace()
    return user


def _fake_request():
    return SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))


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
    item.reserved_stock_mt = Decimal("0")
    item.price_per_mt_usd = Decimal("850")
    item.is_certified = True
    item.certification_declared = True
    item.certification_scheme = "ISCC EU"
    item.specification_standard = "IMPCA"
    item.msds_available = True
    item.carbon_intensity_gco2_mj = Decimal("21.5")
    item.carbon_intensity_method = "ISCC EU"
    item.feedstock = "Biogenic CO2"
    item.origin = "Iceland"
    item.off_spec = True
    item.off_spec_notes = "Water content under review"
    return item


class TestPublishInventoryItem:
    @pytest.mark.asyncio
    async def test_publish_maps_catalog_product_and_delivery_point(self):
        user = _make_user()
        item = _make_inventory_item()

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        item_result = MagicMock()
        item_result.scalar_one_or_none.return_value = item

        user_result = MagicMock()
        user_result.scalar_one_or_none.return_value = user
        organization = SimpleNamespace(id=user.organization_id, verification_status="APPROVED")
        organization_result = MagicMock()
        organization_result.scalar_one_or_none.return_value = organization

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
            user_result,
            organization_result,
            item_result,
            exact_product_result,
            product_result,
            dp_result,
        ]

        result = await publish_inventory_item(
            item_id=item.id,
            request=_fake_request(),
            db=mock_db,
            current_user=user,
        )

        assert result["status"] == "published"
        listing = mock_db.add.call_args_list[0].args[0]
        assert listing.product_id == fallback_product.id
        assert listing.owner_user_id == user.id
        assert listing.inventory_item_id == item.id
        assert listing.delivery_point_id == delivery_point.id
        assert listing.port_id == item.port_id
        assert listing.side.value == "ASK"
        assert listing.quantity_mt == Decimal("1000")
        assert listing.remaining_quantity_mt == Decimal("1000")
        assert listing.price_per_mt_usd == Decimal("850")
        assert listing.certification_declared is True
        assert listing.certification_scheme == "ISCC EU"
        assert listing.specification_standard == "IMPCA"
        assert listing.msds_available is True
        assert listing.carbon_intensity_gco2_mj == Decimal("21.5")
        assert listing.carbon_intensity_method == "ISCC EU"
        assert listing.feedstock == "Biogenic CO2"
        assert listing.origin == "Iceland"
        assert listing.off_spec is True
        assert listing.off_spec_notes == "Water content under review"
        assert item.reserved_stock_mt == Decimal("1000")
        mock_db.commit.assert_awaited_once()
        mock_db.refresh.assert_awaited_once()


class TestListingPayload:
    def test_listing_payload_includes_supplier_metadata(self):
        order = MagicMock()
        order.id = uuid4()
        order.product_name = "Bio Methanol"
        order.market_product = "BIO_METHANOL"
        order.fuel_type = "Methanol"
        order.fuel_grade = "Bio"
        order.quantity_mt = Decimal("1000")
        order.price_per_mt_usd = Decimal("850")
        order.region = "Singapore"
        order.availability_window = "SPOT"
        order.certifications = ["ISCC"]
        order.certification_declared = True
        order.certification_scheme = "ISCC EU"
        order.specification_standard = "IMPCA"
        order.msds_available = True
        order.carbon_intensity_gco2_mj = Decimal("21.5")
        order.carbon_intensity_method = "ISCC EU"
        order.feedstock = "Biogenic CO2"
        order.origin = "Iceland"
        order.off_spec = True
        order.off_spec_notes = "Water content under review"
        order.status = OrderBookStatus.OPEN

        payload = _listing_payload(order, match_count=2)

        assert payload["product_name"] == "Bio Methanol"
        assert payload["market_product"] == "BIO_METHANOL"
        assert payload["fuel_grade"] == "Bio"
        assert payload["availability_window"] == "SPOT"
        assert payload["certifications"] == ["ISCC"]
        assert payload["certification_declared"] is True
        assert payload["certification_scheme"] == "ISCC EU"
        assert payload["specification_standard"] == "IMPCA"
        assert payload["msds_available"] is True
        assert payload["carbon_intensity_gco2_mj"] == "21.5"
        assert payload["carbon_intensity_method"] == "ISCC EU"
        assert payload["feedstock"] == "Biogenic CO2"
        assert payload["origin"] == "Iceland"
        assert payload["off_spec"] is True
        assert payload["off_spec_notes"] == "Water content under review"
        assert payload["match_count"] == 2

"""Unit tests for inventory publishing helpers."""
import pytest
from pydantic import ValidationError
from unittest.mock import AsyncMock, MagicMock
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

from app.routers.inventory import (
    _listing_payload,
    _resolve_catalog_product,
    list_inventory,
    list_my_listings,
    list_public_listings,
    publish_inventory_item,
)
from app.models.marketplace import FuelType as ModelFuelType
from app.models.orderbook import OrderBookStatus
from app.models.user import UserRole, UserStatus
from app.schemas.marketplace import InventoryCreate, InventoryItemUpdate


def _make_user():
    user = MagicMock()
    user.id = uuid4()
    user.role = UserRole.SUPPLIER
    user.organization_id = uuid4()
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
    item.off_spec = False
    item.off_spec_notes = None
    return item


class TestPublishInventoryItem:
    @pytest.mark.asyncio
    async def test_publish_maps_exact_canonical_catalog_product_and_delivery_point(self, monkeypatch):
        monkeypatch.setattr("app.config.settings.AUTO_MATCHING_ENABLED", False)
        user = _make_user()
        item = _make_inventory_item()
        monkeypatch.setattr("app.routers.inventory.reserve_inventory", AsyncMock(return_value=item))
        organization = MagicMock(
            provenance="UNKNOWN", verification_status="APPROVED"
        )
        organization.id = user.organization_id
        # The in-transaction supplier re-lock re-reads and revalidates the
        # concrete admitted user; the locked row must satisfy the security
        # eligibility predicate.
        locked_user = MagicMock()
        locked_user.id = user.id
        locked_user.organization_id = user.organization_id
        locked_user.role = UserRole.SUPPLIER
        locked_user.status = UserStatus.APPROVED
        locked_user.email_verified = True
        locked_user.must_change_password = False
        locked_user.kyc_status = "APPROVED"
        locked_user.kyc_organization_id = None
        monkeypatch.setattr(
            "app.routers.inventory.expire_market_slice_orders",
            AsyncMock(return_value=([], {user.organization_id: organization})),
        )
        monkeypatch.setattr(
            "app.routers.inventory._best_slice_price",
            AsyncMock(return_value=None),
        )
        monkeypatch.setattr(
            "app.routers.inventory.rebuild_live_slice_benchmarks_for_keys",
            AsyncMock(),
        )
        monkeypatch.setattr("app.routers.inventory.emit_order_created", AsyncMock())
        monkeypatch.setattr(
            "app.routers.inventory.collect_auto_match_side_effects",
            AsyncMock(return_value=[]),
        )
        monkeypatch.setattr(
            "app.routers.inventory.enqueue_market_events",
            AsyncMock(),
        )
        monkeypatch.setattr("app.routers.inventory.track_analytics_event", MagicMock())
        monkeypatch.setattr(
            "app.routers.inventory.lock_and_load_market_organizations",
            AsyncMock(
                return_value={
                    user.organization_id: organization
                }
            ),
        )

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        item_result = MagicMock()
        item_result.scalar_one_or_none.return_value = item

        exact_product_result = MagicMock()
        catalog_product = MagicMock()
        catalog_product.id = uuid4()
        catalog_product.name = "Bio Methanol"
        catalog_product.fuel_type = "Methanol"
        catalog_product.fuel_grade = "Bio"
        exact_product_result.scalar_one_or_none.return_value = catalog_product
        item.product_name = catalog_product.name

        delivery_point = MagicMock()
        delivery_point.id = uuid4()
        delivery_point.name = "Singapore"
        dp_result = MagicMock()
        dp_result.scalar_one_or_none.return_value = delivery_point

        mock_db.execute.side_effect = [
            MagicMock(scalar_one_or_none=MagicMock(return_value=locked_user)),
            MagicMock(scalar_one_or_none=MagicMock(return_value=organization)),
            item_result,
            MagicMock(scalar_one_or_none=MagicMock(return_value=None)),
            exact_product_result,
            dp_result,
            item_result,
            exact_product_result,
            dp_result,
            MagicMock(scalar_one_or_none=MagicMock(return_value=None)),
            MagicMock(scalar_one_or_none=MagicMock(return_value=None)),
            MagicMock(scalar_one_or_none=MagicMock(return_value=MagicMock(provenance="UNKNOWN"))),
        ]

        result = await publish_inventory_item(
            item_id=item.id,
            request=_fake_request(),
            db=mock_db,
            current_user=user,
        )

        assert result["status"] == "published"
        listing = mock_db.add.call_args_list[0].args[0]
        assert listing.owner_user_id == user.id
        assert listing.product_id == catalog_product.id
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
        assert listing.off_spec is False
        assert listing.off_spec_notes is None
        mock_db.commit.assert_awaited_once()
        mock_db.refresh.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_catalog_resolution_never_falls_back_to_generic_fuel_taxonomy(self):
        item = _make_inventory_item()
        item.product_name = "Methanol Green"
        db = AsyncMock()
        missing = MagicMock()
        missing.scalar_one_or_none.return_value = None
        db.execute.return_value = missing

        assert await _resolve_catalog_product(db, item) is None
        assert db.execute.await_count == 1


class TestInventoryCollectionBounds:
    @staticmethod
    def _empty_result():
        result = MagicMock()
        result.unique.return_value.scalars.return_value.all.return_value = []
        result.scalars.return_value.all.return_value = []
        return result

    @pytest.mark.asyncio
    async def test_public_listing_alias_applies_sql_limit_before_loading(self):
        db = AsyncMock()
        db.execute.return_value = self._empty_result()

        assert await list_public_listings(db=db, skip=5, limit=17) == []

        statement = db.execute.await_args.args[0]
        assert statement._offset_clause.value == 5
        assert statement._limit_clause.value == 17

    @pytest.mark.asyncio
    async def test_my_listing_alias_applies_sql_limit_before_trade_eager_load(self):
        db = AsyncMock()
        db.execute.return_value = self._empty_result()
        user = _make_user()

        assert await list_my_listings(db=db, current_user=user, skip=3, limit=11) == []

        statement = db.execute.await_args.args[0]
        assert statement._offset_clause.value == 3
        assert statement._limit_clause.value == 11

    @pytest.mark.asyncio
    async def test_supplier_inventory_collection_is_bounded_in_sql(self):
        db = AsyncMock()
        db.execute.return_value = self._empty_result()
        user = _make_user()

        assert await list_inventory(db=db, current_user=user, skip=2, limit=13) == []

        statement = db.execute.await_args.args[0]
        assert statement._offset_clause.value == 2
        assert statement._limit_clause.value == 13


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


class TestInventoryWriteContract:
    def test_create_rejects_server_managed_reserved_stock(self):
        with pytest.raises(ValidationError, match="reserved_stock_mt"):
            InventoryCreate(
                port_id="SGSIN",
                fuel_type="Methanol",
                current_stock_mt="100.00",
                reserved_stock_mt="1.00",
            )

    def test_update_rejects_server_managed_reserved_stock(self):
        with pytest.raises(ValidationError, match="reserved_stock_mt"):
            InventoryItemUpdate(reserved_stock_mt="1.00")

    def test_inventory_link_is_restrictive(self):
        from app.models.orderbook import OrderBookOrder

        fk = next(iter(OrderBookOrder.__table__.c.inventory_item_id.foreign_keys))
        assert fk.ondelete in {None, "RESTRICT"}

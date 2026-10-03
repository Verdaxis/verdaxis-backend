"""Unit tests for orderbook router guardrails."""
import pytest
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from unittest.mock import AsyncMock, MagicMock, Mock
from types import SimpleNamespace
from uuid import uuid4

from fastapi import HTTPException

from app.models.user import UserRole
from app.routers.orderbook import (
    _amendment_loses_priority,
    create_order,
    latest_supplier_listing_template,
)
from app.models.orderbook import OrderBookOrder
from app.schemas.orderbook import OrderCreate, OrderSide


def _make_buyer_user():
    user = MagicMock()
    user.id = uuid4()
    user.role = UserRole.BUYER
    user.organization_id = uuid4()
    return user


def _make_supplier_user():
    user = MagicMock()
    user.id = uuid4()
    user.role = UserRole.SUPPLIER
    user.organization_id = uuid4()
    return user


def _fake_request():
    return SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))


def _priority_order(**overrides):
    values = {
        "price_per_mt_usd": Decimal("550"),
        "quantity_mt": Decimal("1000"),
        "expires_at": datetime.now(UTC) + timedelta(days=1),
        "certifications": ["ISCC EU"],
        "certification_scheme": "ISCC EU",
        "fame_terms": None,
        "off_spec": False,
    }
    values.update(overrides)
    return SimpleNamespace(**values)


@pytest.mark.parametrize(
    "changes",
    [
        {"price_per_mt_usd": Decimal("551")},
        {"quantity_mt": Decimal("1001")},
        {"certifications": ["ISCC EU", "REDcert EU"]},
        {"certification_scheme": "REDcert EU"},
        {"fame_terms": {"feedstock": "used_cooking_oil"}},
    ],
)
def test_execution_broadening_loses_priority(changes):
    assert _amendment_loses_priority(_priority_order(), changes)


def test_expiry_extension_loses_priority():
    order = _priority_order()
    assert _amendment_loses_priority(
        order, {"expires_at": order.expires_at + timedelta(days=1)}
    )


@pytest.mark.parametrize(
    "changes",
    [
        {"quantity_mt": Decimal("900")},
        {"price_per_mt_usd": Decimal("550")},
        {"certifications": [" iscc eu "]},
        {"origin": "updated disclosure"},
    ],
)
def test_reduction_and_noop_or_unrelated_changes_retain_priority(changes):
    assert not _amendment_loses_priority(_priority_order(), changes)


class TestCreateOrder:
    @pytest.mark.asyncio
    async def test_bid_can_omit_certification_scheme(self):
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
        )

        assert order.certification_scheme is None

    @pytest.mark.asyncio
    async def test_bid_rejects_supplier_metadata(self):
        order = OrderCreate(
            side=OrderSide.BID,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            certification_scheme="ISCC EU",
            certification_declared=True,
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                request=_fake_request(),
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
            certification_scheme="ISCC EU",
            off_spec=False,
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                request=_fake_request(),
                order_data=order,
                current_user=_make_buyer_user(),
                db=AsyncMock(),
            )

        assert exc_info.value.status_code == 400
        assert "off_spec" in exc_info.value.detail


    @pytest.mark.asyncio
    async def test_bid_persists_certification_preferences(self, monkeypatch):
        product_id = uuid4()
        delivery_point_id = uuid4()
        product = MagicMock(
            id=product_id,
            market_product='BIO_METHANOL',
            min_lot_size=Decimal('200.00'),
            unit='MT',
            name='Bio Methanol',
        )
        delivery_point = MagicMock(id=delivery_point_id)

        product_result = MagicMock()
        product_result.scalars.return_value.first.return_value = product
        delivery_point_result = MagicMock()
        delivery_point_result.scalars.return_value.first.return_value = delivery_point
        user = _make_buyer_user()
        monkeypatch.setattr(
            "app.routers.orderbook.lock_and_load_market_organizations",
            AsyncMock(
                return_value={
                    user.organization_id: MagicMock(
                        provenance="UNKNOWN", verification_status="APPROVED"
                    )
                }
            ),
        )
        monkeypatch.setattr(
            "app.routers.orderbook._best_slice_price", AsyncMock(return_value=None)
        )

        captured: list[OrderBookOrder] = []
        db = AsyncMock()
        db.execute.side_effect = [product_result, delivery_point_result]
        db.add = Mock(side_effect=lambda obj: captured.append(obj))
        db.flush.side_effect = RuntimeError('stop-after-add')

        order = OrderCreate(
            side=OrderSide.BID,
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            quantity_mt=Decimal('1000'),
            price_per_mt_usd=Decimal('550'),
            certifications=[' ISCC EU ', 'REDcert EU', 'ISCC EU'],
        )

        with pytest.raises(RuntimeError, match='stop-after-add'):
            await create_order(
                request=_fake_request(),
                order_data=order,
                current_user=user,
                db=db,
            )

        assert captured
        assert captured[0].certification_scheme is None
        assert captured[0].certifications == ['ISCC EU', 'REDcert EU']

    @pytest.mark.asyncio
    async def test_ask_requires_certification_declaration(self):
        order = OrderCreate(
            side=OrderSide.ASK,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            certification_scheme="ISCC EU",
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                request=_fake_request(),
                order_data=order,
                current_user=_make_supplier_user(),
                db=AsyncMock(),
            )

        assert exc_info.value.status_code == 400
        assert "certification declaration" in exc_info.value.detail.lower()


    @pytest.mark.asyncio
    async def test_ask_requires_supplier_metadata_fields(self):
        order = OrderCreate(
            side=OrderSide.ASK,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            certification_scheme="ISCC EU",
            certification_declared=True,
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                request=_fake_request(),
                order_data=order,
                current_user=_make_supplier_user(),
                db=AsyncMock(),
            )

        assert exc_info.value.status_code == 400
        assert "supplier details" in exc_info.value.detail.lower()
        assert "origin" in exc_info.value.detail.lower()

    @pytest.mark.asyncio
    async def test_ask_requires_certification_scheme(self):
        order = OrderCreate(
            side=OrderSide.ASK,
            product_id=uuid4(),
            delivery_point_id=uuid4(),
            quantity_mt=Decimal("1000"),
            price_per_mt_usd=Decimal("550"),
            certification_declared=True,
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_order(
                request=_fake_request(),
                order_data=order,
                current_user=_make_supplier_user(),
                db=AsyncMock(),
            )

        assert exc_info.value.status_code == 400
        assert "certification scheme" in exc_info.value.detail.lower()


class TestLatestSupplierListingTemplate:
    @pytest.mark.asyncio
    async def test_returns_latest_ask_template_with_off_spec_reset(self):
        latest_order = MagicMock()
        latest_order.product_id = uuid4()
        latest_order.delivery_point_id = uuid4()
        latest_order.quantity_mt = Decimal("2500")
        latest_order.price_per_mt_usd = Decimal("625")
        latest_order.availability_window = "2026-04"
        latest_order.certifications = ["ISCC EU"]
        latest_order.certification_declared = True
        latest_order.certification_scheme = "ISCC EU"
        latest_order.specification_standard = "IMPCA"
        latest_order.msds_available = True
        latest_order.carbon_intensity_gco2_mj = Decimal("12.4")
        latest_order.carbon_intensity_method = "LCFS"
        latest_order.feedstock = "Waste residue"
        latest_order.origin = "Brazil"
        latest_order.off_spec = True
        latest_order.off_spec_notes = "Do not carry"
        latest_order.fame_terms = None

        result = MagicMock()
        result.scalars.return_value.first.return_value = latest_order
        db = AsyncMock()
        db.execute.return_value = result

        payload = await latest_supplier_listing_template(
            current_user=_make_supplier_user(),
            db=db,
        )

        assert payload is not None
        assert payload.product_id == latest_order.product_id
        assert payload.certification_scheme == "ISCC EU"
        assert payload.off_spec is False
        assert payload.off_spec_notes is None

    @pytest.mark.asyncio
    async def test_returns_none_when_supplier_has_no_asks(self):
        result = MagicMock()
        result.scalars.return_value.first.return_value = None
        db = AsyncMock()
        db.execute.return_value = result

        payload = await latest_supplier_listing_template(
            current_user=_make_supplier_user(),
            db=db,
        )

        assert payload is None

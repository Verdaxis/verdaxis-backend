"""Production scalar biofuels coexist with staging's typed products."""
from decimal import Decimal
from uuid import UUID

import pytest
from fastapi import HTTPException

from app.market_catalog import CANONICAL_DELIVERY_POINTS, PRODUCT_IDS, PRODUCTS_BY_CODE
from app.models.orderbook import OrderBookOrder, OrderSide
from app.schemas.preferences import MarketWatchPreferences
from app.services.execution_policy import (
    order_is_execution_qualified,
    orders_execution_compatible,
)

BIOFUELS = {
    "B30": ("c4cecebc-3d2d-5840-8021-57a9a11bc673", "ISO 8217:2024 RF 380"),
    "B100": ("74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5", "ISO 8217:2024 DFA"),
}


def biofuel_order(code, **changes):
    identity, standard = BIOFUELS[code]
    values = dict(
        product_id=UUID(identity), side=OrderSide.ASK, off_spec=False,
        specification_standard=standard, carbon_intensity_method="Whole supplied fuel lifecycle",
        carbon_intensity_gco2_mj=Decimal(0), msds_available=True,
        certification_declared=True, certification_scheme="ISCC EU",
        feedstock="Used cooking oil", origin="Singapore",
    )
    values.update(changes)
    return OrderBookOrder(**values)


@pytest.mark.parametrize("code", BIOFUELS)
def test_production_identity_and_minimum(code):
    spec = PRODUCTS_BY_CODE[code]
    assert spec.id == UUID(BIOFUELS[code][0])
    assert (spec.name, spec.fuel_type, spec.fuel_grade) == (code, "Biofuel", code)
    assert spec.min_lot_size == Decimal(200)
    assert spec.execution_mode == "ORDERBOOK"
    assert spec.available_delivery_point_ids is None


def test_eight_product_preferences_round_trip():
    codes = ["BIO_METHANOL", "E_METHANOL", "BIO_ETHANOL", "SYNTHETIC_ETHANOL", "B30", "B100", "BIO_LNG", "UCOME_B100"]
    preferences = MarketWatchPreferences(products=codes, portIds=[])
    assert preferences.model_dump(mode="json")["products"] == codes
    assert set(PRODUCT_IDS) == set(codes)
    fame = PRODUCTS_BY_CODE["UCOME_B100"]
    assert fame.id == UUID("e561e43f-d9b2-598e-981c-f1d28d515ddc")
    assert fame.min_lot_size == Decimal(1)
    assert fame.available_delivery_point_ids == (UUID("73835e92-820e-584b-8280-bb61c63aa28e"),)


@pytest.mark.parametrize("code", BIOFUELS)
@pytest.mark.parametrize("changes", [
    {"specification_standard": "IMPCA"}, {"carbon_intensity_method": " \t"},
    {"carbon_intensity_gco2_mj": None}, {"off_spec": True},
    {"msds_available": False}, {"certification_declared": False},
    {"certification_scheme": " "}, {"origin": " "}, {"feedstock": None},
])
def test_take_and_match_refuse_incomplete_scalar_contract(code, changes):
    ask = biofuel_order(code, **changes)
    bid = biofuel_order(code, side=OrderSide.BID)
    assert not order_is_execution_qualified(ask)
    assert not orders_execution_compatible(bid, ask)


@pytest.mark.parametrize("code", BIOFUELS)
def test_zero_ci_and_normalized_specification_remain_qualified(code):
    ask = biofuel_order(code, specification_standard=" \t" + BIOFUELS[code][1].lower() + "\n")
    assert order_is_execution_qualified(ask)
    assert orders_execution_compatible(biofuel_order(code, side=OrderSide.BID), ask)


@pytest.mark.parametrize("code", BIOFUELS)
@pytest.mark.parametrize("field,value", [("specification_standard", "IMPCA"), ("carbon_intensity_method", " ")])
def test_shared_create_amend_and_assisted_metadata_validation(code, field, value):
    from app.routers.orderbook import _require_supplier_metadata

    order = biofuel_order(code, **{field: value})
    with pytest.raises(HTTPException) as rejected:
        _require_supplier_metadata(**{
            name: getattr(order, name) for name in (
                "product_id", "specification_standard", "carbon_intensity_method",
                "carbon_intensity_gco2_mj", "msds_available", "feedstock", "origin",
            )
        })
    assert rejected.value.status_code == 400
    assert field in rejected.value.detail


# Reuse the staging route fixture; it binds approved KYC to each organization.
from tests.unit.test_trade_watchlist_hooks import (
    async_engine, setup_tables, db, _make_org, _make_user, _make_product,
    _make_delivery_point, _make_ask, _fake_request, trades_router,
    OrgType, UserRole, Trade, select, order_terms_digest,
)
from app.market_catalog import BIOFUEL_SPECIFICATION_STANDARDS

@pytest.mark.asyncio
@pytest.mark.parametrize("product_name", ["B30", "B100"])
async def test_biofuel_direct_bid_fill_requires_qualified_ask_but_buyers_can_fill_asks(monkeypatch, db, product_name):
    buyer_org = await _make_org(db, "Biofuel Buyer", OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier_org = await _make_org(db, "Biofuel Supplier", OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db, product_name)
    port = await _make_delivery_point(db)
    bid = await _make_ask(
        db, org_id=buyer_org.id, product_id=product.id,
        delivery_point_id=port.id, owner_user_id=buyer.id,
    )
    bid.side = OrderSide.BID
    await db.commit()

    with pytest.raises(HTTPException, match="Place a qualified ASK") as failure:
        await trades_router.create_trade(
            payload=trades_router.TradeCreate(
                order_id=bid.id,
                quantity_mt=Decimal("100"),
                expected_terms_digest=order_terms_digest(bid),
            ),
            request=_fake_request(), db=db, current_user=supplier,
        )
    assert failure.value.status_code == 400
    assert bid.remaining_quantity_mt == Decimal("1000")
    assert (await db.execute(select(Trade))).scalars().all() == []

    ask = await _make_ask(
        db, org_id=supplier_org.id, product_id=product.id,
        delivery_point_id=port.id, owner_user_id=supplier.id,
    )
    ask.specification_standard = BIOFUEL_SPECIFICATION_STANDARDS[product.id]
    ask.carbon_intensity_gco2_mj = Decimal("70")
    ask.carbon_intensity_method = "Lifecycle calculation for the whole supplied fuel"
    ask.feedstock = "Used cooking oil FAME"
    ask.origin = "Singapore"
    await db.commit()

    async def no_notification(*args, **kwargs):
        pass

    monkeypatch.setattr(trades_router, "notify_org_users", no_notification)
    response = await trades_router.create_trade(
        payload=trades_router.TradeCreate(
            order_id=ask.id,
            quantity_mt=Decimal("100"),
            expected_terms_digest=order_terms_digest(ask),
        ),
        request=_fake_request(), db=db, current_user=buyer,
    )
    assert response.status == "PENDING_CONFIRMATION"
    assert response.market_product == product_name
    assert ask.remaining_quantity_mt == Decimal("900")




@pytest.mark.asyncio
@pytest.mark.parametrize("product_name", ["B30", "B100"])
async def test_qualified_zero_ci_scalar_ask_matches_with_exact_identity_and_economics(
    monkeypatch, db, product_name
):
    """A qualified zero-CI scalar ASK fills at its exact product and economics."""
    buyer_org = await _make_org(db, "Scalar Match Buyer", OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier_org = await _make_org(db, "Scalar Match Supplier", OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db, product_name)
    port = await _make_delivery_point(db)
    ask = await _make_ask(
        db, org_id=supplier_org.id, product_id=product.id,
        delivery_point_id=port.id, owner_user_id=supplier.id,
    )
    ask.specification_standard = BIOFUEL_SPECIFICATION_STANDARDS[product.id]
    ask.carbon_intensity_method = "Lifecycle calculation for the whole supplied fuel"
    ask.carbon_intensity_gco2_mj = Decimal("0")
    ask.feedstock = "Used cooking oil"
    ask.origin = "Singapore"
    await db.commit()

    async def no_notification(*args, **kwargs):
        pass

    monkeypatch.setattr(trades_router, "notify_org_users", no_notification)
    response = await trades_router.create_trade(
        payload=trades_router.TradeCreate(
            order_id=ask.id,
            quantity_mt=Decimal("100"),
            expected_terms_digest=order_terms_digest(ask),
        ),
        request=_fake_request(), db=db, current_user=buyer,
    )

    assert response.status == "PENDING_CONFIRMATION"
    assert response.product_id == UUID(BIOFUELS[product_name][0])
    assert response.market_product == product_name
    assert response.quantity_mt == Decimal("100")
    assert response.price_per_mt_usd == Decimal("1090")
    assert ask.remaining_quantity_mt == Decimal("900")




@pytest.mark.parametrize("code", BIOFUELS)
def test_minimum_quantity_applies_to_new_orders(code):
    from app.services.order_quantity import require_minimum_order_quantity
    from tests.unit.test_fame_catalog_execution import _product

    product = _product(code)
    require_minimum_order_quantity(product=product, quantity_mt=Decimal("200"))
    with pytest.raises(HTTPException):
        require_minimum_order_quantity(product=product, quantity_mt=Decimal("199.99"))


def test_delivery_point_tuples_match_pinned_production():
    assert [(str(point.id), point.name, point.region, point.timezone)
            for point in CANONICAL_DELIVERY_POINTS] == [
        ("78281fb3-e726-5396-802e-34b01fda21e8", "Dalian", "Asia", "Asia/Shanghai"),
        ("262d36ae-6f35-5785-b9e8-9e221b0f1b78", "Busan", "Asia", "Asia/Seoul"),
        ("633c0593-f9b9-52ad-abe5-a0285f6888d3", "Shanghai", "Asia", "Asia/Shanghai"),
        ("73835e92-820e-584b-8280-bb61c63aa28e", "Singapore", "Asia", "Asia/Singapore"),
        ("1379d36c-1ca9-55b7-9c0d-5235a0ba1f36", "Rotterdam", "Europe", "Europe/Amsterdam"),
        ("a083db06-b050-56c2-a274-3897eac2fdae", "Houston", "Americas", "America/Chicago"),
        ("1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534", "Los Angeles", "Americas", "America/Los_Angeles"),
        ("a4762444-4e0b-5bc1-b647-a185047adae5", "Santos", "Americas", "America/Sao_Paulo"),
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize("code", BIOFUELS)
@pytest.mark.parametrize("field,value", [
    ("specification_standard", "IMPCA"), ("carbon_intensity_method", " "),
    ("carbon_intensity_gco2_mj", None), ("off_spec", True),
    ("msds_available", False), ("certification_declared", False),
])
async def test_automatic_matching_and_assisted_post_only_refuse_bad_biofuel(monkeypatch, code, field, value):
    from unittest.mock import AsyncMock
    from app.models.user import Organization, OrganizationProvenance
    from app.services.matching_engine import match_order
    from app.services.market_support_post_only import assess_locked_order
    from app.market_catalog import DELIVERY_POINT_IDS
    from uuid import uuid4

    organization = Organization(id=uuid4(), verification_status="APPROVED")
    order = biofuel_order(
        code, **{field: value}, organization_id=organization.id,
        remaining_quantity_mt=Decimal("200"), price_per_mt_usd=Decimal("800"),
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"], availability_window="SPOT",
        provenance=OrganizationProvenance.REAL,
    )
    monkeypatch.setattr("app.services.matching_engine.acquire_market_slice_lock", AsyncMock())
    session = AsyncMock()
    assert await match_order(session, order) == []
    session.execute.assert_not_awaited()
    assessment = await assess_locked_order(session, order, organization=organization)
    assert assessment.indeterminate and assessment.would_cross
    session.execute.assert_not_awaited()


@pytest.mark.asyncio
@pytest.mark.parametrize("code,field,value", [
    ("B30", "specification_standard", "IMPCA"),
    ("B100", "carbon_intensity_method", " \t"),
])
async def test_assisted_authorization_refuses_invalid_scalar_metadata(monkeypatch, code, field, value):
    """The assisted entry path must refuse the same invalid metadata it persists."""
    from datetime import UTC, datetime, timedelta
    from types import SimpleNamespace
    from unittest.mock import AsyncMock, MagicMock
    from uuid import uuid4

    from app.market_catalog import DELIVERY_POINT_IDS
    from app.models.user import User, UserRole, UserStatus
    from app.routers.market_support import create_authorization
    from app.schemas.market_support import AuthorizationCreate
    from app.schemas.orderbook import OrderCreate

    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", False)
    identity, standard = BIOFUELS[code]
    now = datetime.now(UTC)
    order = dict(
        side="ASK", product_id=UUID(identity),
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
        quantity_mt=Decimal("200"), price_per_mt_usd=Decimal("800"),
        availability_window="SPOT", expires_at=now + timedelta(days=1),
        certification_declared=True, certification_scheme="ISCC EU",
        specification_standard=standard,
        carbon_intensity_method="Whole supplied fuel lifecycle",
        carbon_intensity_gco2_mj=Decimal("0"),  # zero CI is a valid control value
        msds_available=True, feedstock="Used cooking oil", origin="Singapore",
    )
    order[field] = value
    body = AuthorizationCreate(
        accountable_user_id=uuid4(),
        order=OrderCreate(**order),
        authorization_expires_at=now + timedelta(hours=12),
        evidence_reference="assisted-case-1",
        evidence_sha256="a" * 64,
        commercial_consent_version="v1",
        commercial_consent_reference="consent-1",
    )
    admin = User(
        id=uuid4(), email="assisted-admin@example.com", password_hash="unused",
        role=UserRole.ADMIN, status=UserStatus.APPROVED, email_verified=True,
    )
    capability_result = MagicMock()
    capability_result.scalar_one_or_none.return_value = MagicMock(revoked_at=None, expires_at=None)
    missing_result = MagicMock()
    missing_result.scalar_one_or_none.return_value = None
    db = AsyncMock()
    db.add = MagicMock()
    db.get_bind = MagicMock(return_value=SimpleNamespace(dialect=SimpleNamespace(name="sqlite")))
    db.execute.side_effect = [capability_result, missing_result]

    with pytest.raises(HTTPException) as rejected:
        await create_authorization(
            organization_id=uuid4(), request=_fake_request(), body=body, db=db,
            current_user=admin, idempotency_key_header="assisted-test-key-1",
        )

    assert rejected.value.status_code == 400
    assert code in rejected.value.detail
    db.add.assert_not_called()
    db.commit.assert_not_awaited()


@pytest.mark.parametrize("code", BIOFUELS)
def test_demo_and_benchmark_definitions_preserve_scalar_contract(code):
    from datetime import UTC, datetime
    from app.seeds.market_seed import PRICING, CI_DATA, ask_seed_metadata
    from app.seeds.forward_monitoring_seed import _demo_slices
    from app.services.benchmarks import _base_benchmark_price

    assert code in PRICING and code in CI_DATA
    assert _base_benchmark_price(code, "Singapore") is not None
    assert (code, "Singapore", "SPOT") in _demo_slices(datetime(2026, 10, 16, tzinfo=UTC))
    metadata = ask_seed_metadata("ISCC EU", product_name=code)
    assert metadata["specification_standard"] == BIOFUELS[code][1]
    assert "whole-" in metadata["carbon_intensity_method"]
    assert not {"Bio-LNG", "UCOME B100"} & PRICING.keys()
    assert _base_benchmark_price("BIO_LNG", "Rotterdam") is None
    assert _base_benchmark_price("UCOME_B100", "Singapore") is None


@pytest.mark.asyncio
@pytest.mark.parametrize("code", BIOFUELS)
@pytest.mark.parametrize("field,value", [("specification_standard", "IMPCA"), ("carbon_intensity_method", " ")])
async def test_create_and_amend_routes_reject_invalid_scalar_contract(db, code, field, value):
    from unittest.mock import AsyncMock
    from app.routers.orderbook import create_order, update_order
    from app.schemas.orderbook import OrderCreate, OrderUpdate
    from tests.unit.test_orderbook_router import _make_supplier_user
    from app.market_catalog import DELIVERY_POINT_IDS

    metadata = {name: getattr(biofuel_order(code), name) for name in (
        "certification_declared", "certification_scheme", "specification_standard",
        "msds_available", "carbon_intensity_gco2_mj", "carbon_intensity_method",
        "feedstock", "origin",
    )}
    metadata[field] = value
    create_db = AsyncMock()
    with pytest.raises(HTTPException) as created:
        await create_order(
            request=_fake_request(), current_user=_make_supplier_user(), db=create_db,
            order_data=OrderCreate(
                side="ASK", product_id=PRODUCT_IDS[code],
                delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
                quantity_mt=Decimal("200"), price_per_mt_usd=Decimal("800"), **metadata,
            ),
        )
    assert created.value.status_code == 400 and field in created.value.detail
    create_db.commit.assert_not_awaited()

    organization = await _make_org(db, "Amend Supplier", OrgType.FUEL_SUPPLIER)
    user = await _make_user(db, organization, UserRole.SUPPLIER)
    product = await _make_product(db, code)
    point = await _make_delivery_point(db)
    order = await _make_ask(db, org_id=organization.id, owner_user_id=user.id,
                            product_id=product.id, delivery_point_id=point.id)
    for name in metadata:
        setattr(order, name, getattr(biofuel_order(code), name))
    await db.commit()
    with pytest.raises(HTTPException) as amended:
        await update_order(order.id, request=_fake_request(), current_user=user, db=db,
                           update_data=OrderUpdate(**{field: value}))
    assert amended.value.status_code == 400 and field in amended.value.detail
    assert order.specification_standard == BIOFUELS[code][1]
